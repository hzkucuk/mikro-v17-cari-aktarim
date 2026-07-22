//! Mikro V17 Cari Kartı Aktarma — Tauri komut katmanı.

mod db;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use db::{DbConfig, RowStatus, TransferRow, TransferSummary, TriggerCfg};
use tauri::{Emitter, Manager, Window};

#[derive(Default)]
pub struct AppState {
    /// "İptal" butonu bunu set eder; aktarım döngüsü satır aralarında kontrol eder.
    cancel: Arc<AtomicBool>,
}

// ---------------------------------------------------------------------------
// Komutlar
// ---------------------------------------------------------------------------

#[tauri::command]
async fn test_connection(cfg: DbConfig) -> Result<String, String> {
    let mut client = db::connect(&cfg).await?;
    let mut info = db::probe(&mut client).await?;

    if db::check_sp_exists(&mut client).await? {
        info.push_str("\n\n✓ dbo.msp_CariKodunuDegistir bulundu.");
    } else {
        info.push_str(
            "\n\n⚠ UYARI: dbo.msp_CariKodunuDegistir bu veritabanında BULUNAMADI. \
             Aktarım çalışmayacaktır. Doğru Mikro veritabanına bağlandığınızdan emin olun.",
        );
    }

    Ok(info)
}

/// Trigger'ın mevcut durumunu (etkin/devre dışı) döner — UI'da göstermek için.
#[tauri::command]
async fn trigger_status(cfg: DbConfig, trigger: TriggerCfg) -> Result<String, String> {
    let name = trigger
        .name
        .trim()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .trim_matches(|c| c == '[' || c == ']')
        .to_string();

    if name.is_empty() {
        return Ok("Trigger tanımlı değil.".into());
    }

    let mut client = db::connect(&cfg).await?;

    let rows = client
        .query(
            "SELECT is_disabled FROM sys.triggers WHERE name = @P1",
            &[&name],
        )
        .await
        .map_err(|e| format!("Trigger sorgulanamadı: {e}"))?
        .into_first_result()
        .await
        .map_err(|e| format!("Trigger sonucu okunamadı: {e}"))?;

    match rows.into_iter().next().and_then(|r| r.get::<bool, _>(0)) {
        Some(true) => Ok(format!("'{name}' şu anda DEVRE DIŞI.")),
        Some(false) => Ok(format!("'{name}' şu anda ETKİN.")),
        None => Err(format!("'{name}' adlı trigger bulunamadı.")),
    }
}

#[tauri::command]
fn cancel_transfer(state: tauri::State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn run_transfer(
    cfg: DbConfig,
    trigger: TriggerCfg,
    rows: Vec<TransferRow>,
    cari_tipi: i32,
    user_id: i32,
    son_deg_guncelle: bool,
    window: Window,
    state: tauri::State<'_, AppState>,
) -> Result<TransferSummary, String> {
    if rows.is_empty() {
        return Err("Aktarılacak satır yok.".into());
    }

    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::SeqCst);

    // Trigger adları DDL'e string olarak gömülecek — önce katı doğrulama.
    // Boş bırakılırsa trigger yönetimi tamamen atlanır.
    let manage_trigger = !trigger.name.trim().is_empty() && !trigger.table.trim().is_empty();

    let (trig_ident, table_ident) = if manage_trigger {
        (
            db::validate_qualified_ident(&trigger.name)?,
            db::validate_qualified_ident(&trigger.table)?,
        )
    } else {
        (String::new(), String::new())
    };

    let mut client = db::connect(&cfg).await?;

    if !db::check_sp_exists(&mut client).await? {
        return Err("dbo.msp_CariKodunuDegistir bu veritabanında bulunamadı. \
             Aktarım iptal edildi — yanlış veritabanına bağlanmış olabilirsiniz."
            .into());
    }

    // --- Trigger'ı kapat ---------------------------------------------------
    if manage_trigger {
        db::set_trigger(&mut client, &trig_ident, &table_ident, false).await?;
        let _ = window.emit(
            "log",
            format!("Trigger devre dışı bırakıldı: {trig_ident} ON {table_ident}"),
        );
    }

    // --- Satırları işle ----------------------------------------------------
    // Bu blok ne olursa olsun tamamlanır; ardından trigger MUTLAKA geri açılır.
    let mut ok = 0usize;
    let mut failed = 0usize;
    let mut errors: Vec<String> = Vec::new();
    let total = rows.len();

    for (index, row) in rows.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            let _ = window.emit("log", "İptal edildi — kalan satırlar atlandı.".to_string());
            break;
        }

        let _ = window.emit(
            "row-status",
            RowStatus {
                index,
                eski: row.eski.clone(),
                yeni: row.yeni.clone(),
                status: "running".into(),
                message: "İşleniyor…".into(),
            },
        );

        match db::transfer_one(&mut client, row, cari_tipi, user_id, son_deg_guncelle).await {
            Ok(msg) => {
                ok += 1;
                let _ = window.emit(
                    "row-status",
                    RowStatus {
                        index,
                        eski: row.eski.clone(),
                        yeni: row.yeni.clone(),
                        status: "ok".into(),
                        message: msg,
                    },
                );
            }
            Err(e) => {
                failed += 1;
                errors.push(format!("{} → {}: {e}", row.eski, row.yeni));
                let _ = window.emit(
                    "row-status",
                    RowStatus {
                        index,
                        eski: row.eski.clone(),
                        yeni: row.yeni.clone(),
                        status: "error".into(),
                        message: e,
                    },
                );
            }
        }

        let _ = window.emit(
            "progress",
            serde_json::json!({ "done": index + 1, "total": total }),
        );
    }

    // --- Trigger'ı MUTLAKA geri aç -----------------------------------------
    let (trigger_restored, trigger_message) = if manage_trigger {
        match db::set_trigger(&mut client, &trig_ident, &table_ident, true).await {
            Ok(()) => {
                let m = format!("Trigger tekrar etkinleştirildi: {trig_ident}");
                let _ = window.emit("log", m.clone());
                (true, m)
            }
            Err(e) => {
                let m = format!(
                    "KRİTİK: Trigger GERİ AÇILAMADI ({e}). \
                     Lütfen SSMS'te şunu çalıştırın: ENABLE TRIGGER {trig_ident} ON {table_ident}"
                );
                let _ = window.emit("trigger-alert", m.clone());
                (false, m)
            }
        }
    } else {
        (true, "Trigger yönetimi atlandı.".to_string())
    };

    Ok(TransferSummary {
        total,
        ok,
        failed,
        trigger_restored,
        trigger_message,
        errors,
    })
}

/// Trigger geri açılamadıysa kullanıcının UI'dan tekrar denemesi için.
#[tauri::command]
async fn enable_trigger(cfg: DbConfig, trigger: TriggerCfg) -> Result<String, String> {
    let trig = db::validate_qualified_ident(&trigger.name)?;
    let table = db::validate_qualified_ident(&trigger.table)?;

    let mut client = db::connect(&cfg).await?;
    db::set_trigger(&mut client, &trig, &table, true).await?;

    Ok(format!("Trigger etkinleştirildi: {trig} ON {table}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(AppState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            test_connection,
            trigger_status,
            run_transfer,
            cancel_transfer,
            enable_trigger,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri uygulaması başlatılamadı");
}
