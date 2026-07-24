//! Mikro V17 Cari Kartı Aktarma — Tauri komut katmanı.

mod db;
mod settings;

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

#[tauri::command]
async fn backup_database(
    cfg: DbConfig,
    backup_directory: String,
) -> Result<db::BackupResult, String> {
    db::backup_database(&cfg, &backup_directory).await
}

/// Trigger'ın mevcut durumunu (etkin/devre dışı) döner — UI'da göstermek için.
#[tauri::command]
async fn trigger_status(cfg: DbConfig, triggers: Vec<TriggerCfg>) -> Result<String, String> {
    if triggers.is_empty() {
        return Ok("Yönetilecek trigger tanımlı değil.".into());
    }
    let mut client = db::connect(&cfg).await?;
    let mut messages = Vec::with_capacity(triggers.len());
    for trigger in triggers {
        let name = trigger
            .name
            .trim()
            .rsplit('.')
            .next()
            .unwrap_or("")
            .trim_matches(|c| c == '[' || c == ']')
            .to_string();
        if name.is_empty() {
            return Err("Boş trigger adı.".into());
        }
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
            Some(true) => messages.push(format!("'{name}' şu anda DEVRE DIŞI.")),
            Some(false) => messages.push(format!("'{name}' şu anda ETKİN.")),
            None => return Err(format!("'{name}' adlı trigger bulunamadı.")),
        }
    }
    Ok(messages.join("\n"))
}

#[tauri::command]
fn cancel_transfer(state: tauri::State<'_, AppState>) {
    state.cancel.store(true, Ordering::SeqCst);
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn run_transfer(
    cfg: DbConfig,
    triggers: Vec<TriggerCfg>,
    rows: Vec<TransferRow>,
    cari_tipi: i32,
    user_id: i32,
    son_deg_guncelle: bool,
    skip_control_key: String,
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
    let trigger_idents: Vec<(String, String)> = triggers
        .iter()
        .map(|trigger| {
            Ok((
                db::validate_qualified_ident(&trigger.name)?,
                db::validate_qualified_ident(&trigger.table)?,
            ))
        })
        .collect::<Result<_, String>>()?;

    let mut client = db::connect(&cfg).await?;

    if !db::check_sp_exists(&mut client).await? {
        return Err("dbo.msp_CariKodunuDegistir bu veritabanında bulunamadı. \
             Aktarım iptal edildi — yanlış veritabanına bağlanmış olabilirsiniz."
            .into());
    }

    db::ensure_audit_log_table(&mut client).await?;
    db::ensure_trigger_guard_table(&mut client).await?;
    let client_machine = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Bilinmiyor".to_string());
    let client_user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(user)) if !domain.is_empty() => format!("{domain}\\{user}"),
        (_, Ok(user)) => user,
        _ => std::env::var("USER").unwrap_or_else(|_| "Bilinmiyor".to_string()),
    };
    let _ = window.emit(
        "log",
        format!("SQL denetim kaydı etkin: {client_machine} / {client_user}"),
    );

    // --- Trigger'ı kapat ---------------------------------------------------
    let mut disabled_triggers: Vec<(String, String, i64)> = Vec::new();
    for (trig_ident, table_ident) in &trigger_idents {
        // Önceden devre dışı bırakılmış trigger'ı uygulama açmaz; yalnızca
        // kendi kapattığı trigger'lar için koruma kaydı tutar.
        if db::trigger_is_disabled(&mut client, trig_ident).await? {
            if let Some(orphan_guard) =
                db::pending_trigger_guard(&mut client, trig_ident, table_ident).await?
            {
                db::set_trigger(&mut client, trig_ident, table_ident, true).await?;
                db::resolve_trigger_guard(
                    &mut client,
                    orphan_guard,
                    "Sonraki çalıştırmada otomatik kurtarıldı",
                )
                .await?;
                let _ = window.emit(
                    "log",
                    format!(
                        "Önceki işlemden açık kalan trigger otomatik geri açıldı: {trig_ident}"
                    ),
                );
            } else {
                let _ = window.emit("log", format!("Trigger zaten devre dışı: {trig_ident}"));
                continue;
            }
        }
        let guard_id = db::register_trigger_guard(
            &mut client,
            trig_ident,
            table_ident,
            &client_machine,
            &client_user,
        )
        .await?;
        if let Err(error) = db::set_trigger(&mut client, trig_ident, table_ident, false).await {
            let _ = db::resolve_trigger_guard(&mut client, guard_id, "Disable başarısız").await;
            for (disabled_trigger, disabled_table, disabled_guard) in disabled_triggers.iter().rev()
            {
                let _ = db::set_trigger(&mut client, disabled_trigger, disabled_table, true).await;
                let _ = db::resolve_trigger_guard(
                    &mut client,
                    *disabled_guard,
                    "Disable zinciri geri alındı",
                )
                .await;
            }
            return Err(format!(
                "'{trig_ident}' devre dışı bırakılamadı: {error}. Önceden kapatılan trigger'lar geri açılmaya çalışıldı."
            ));
        }
        disabled_triggers.push((trig_ident.clone(), table_ident.clone(), guard_id));
        let _ = window.emit(
            "log",
            format!("Trigger devre dışı bırakıldı: {trig_ident} ON {table_ident}"),
        );
    }

    // --- Sipariş kontrolünü BU OTURUMDA atla (session-context) -------------
    // Muhafızlı trigger'lar yalnız bu bağlantıda atlanır; diğer kullanıcıların
    // validasyonu etkilenmez. Global DISABLE'a gerek kalmadan mesai içinde çalışır.
    let skip_key = skip_control_key.trim().to_string();
    let mut skip_control_set = false;
    if !skip_key.is_empty() {
        db::set_skip_control(&mut client, &skip_key, true).await?;
        skip_control_set = true;
        let _ = window.emit(
            "log",
            format!(
                "Sipariş kontrolü bu oturumda atlanıyor (session-context: {skip_key}). Diğer kullanıcılar etkilenmez."
            ),
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

        match db::transfer_one(
            &mut client,
            row,
            cari_tipi,
            user_id,
            son_deg_guncelle,
            &client_machine,
            &client_user,
        )
        .await
        {
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
            Err(mut e) => {
                if let Err(audit_error) = db::write_audit_log(
                    &mut client,
                    db::AuditEntry {
                        client_machine: &client_machine,
                        client_user: &client_user,
                        eski: &row.eski,
                        yeni: &row.yeni,
                        cari_tipi,
                        sil: row.sil,
                        result: "ERROR",
                        message: &e,
                    },
                )
                .await
                {
                    e.push_str(&format!(
                        " | HATA denetim kaydı da yazılamadı: {audit_error}"
                    ));
                }
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

    // --- Session-context bayrağını sıfırla (validasyon geri) ---------------
    if skip_control_set {
        match db::set_skip_control(&mut client, &skip_key, false).await {
            Ok(()) => {
                let _ = window.emit(
                    "log",
                    "Sipariş kontrolü geri açıldı (session-context temizlendi).".to_string(),
                );
            }
            Err(e) => {
                let _ = window.emit(
                    "log",
                    format!("Uyarı: session-context temizlenemedi: {e} (bağlantı kapanınca otomatik silinir)"),
                );
            }
        }
    }

    // --- Trigger'ı MUTLAKA geri aç -----------------------------------------
    let (trigger_restored, trigger_message) = if disabled_triggers.is_empty() {
        (true, "Trigger yönetimi atlandı.".to_string())
    } else {
        let mut restore_errors = Vec::new();
        for (trig_ident, table_ident, guard_id) in disabled_triggers.iter().rev() {
            match db::set_trigger(&mut client, trig_ident, table_ident, true).await {
                Ok(()) => {
                    let _ = db::resolve_trigger_guard(
                        &mut client,
                        *guard_id,
                        "Normal akışta geri açıldı",
                    )
                    .await;
                    let m = format!("Trigger tekrar etkinleştirildi: {trig_ident}");
                    let _ = window.emit("log", m);
                }
                Err(e) => restore_errors.push(format!(
                    "ENABLE TRIGGER {trig_ident} ON {table_ident} ({e})"
                )),
            }
        }
        if restore_errors.is_empty() {
            (true, "Tüm trigger'lar tekrar etkinleştirildi.".to_string())
        } else {
            let m = format!(
                "KRİTİK: Bazı trigger'lar geri açılamadı. SSMS'te çalıştırın:\n{}",
                restore_errors.join("\n")
            );
            let _ = window.emit("trigger-alert", m.clone());
            (false, m)
        }
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
async fn enable_trigger(cfg: DbConfig, triggers: Vec<TriggerCfg>) -> Result<String, String> {
    let mut client = db::connect(&cfg).await?;
    let mut messages = Vec::new();
    for trigger in triggers {
        let trig = db::validate_qualified_ident(&trigger.name)?;
        let table = db::validate_qualified_ident(&trigger.table)?;
        db::set_trigger(&mut client, &trig, &table, true).await?;
        messages.push(format!("Trigger etkinleştirildi: {trig} ON {table}"));
    }
    Ok(messages.join("\n"))
}

/// Aktarım öncesi onay için, çalıştırılacak SQL'in birebir önizlemesini üretir.
/// Kopyalama (sil=false) satırı varsa gerçek kolon listesini canlı bağlantıdan
/// çözer; yalnızca yeniden adlandırma varsa bağlantı gerekmez.
#[tauri::command]
async fn preview_transfer_sql(
    cfg: DbConfig,
    triggers: Vec<TriggerCfg>,
    rows: Vec<TransferRow>,
    cari_tipi: i32,
    user_id: i32,
    son_deg_guncelle: bool,
) -> Result<String, String> {
    if rows.is_empty() {
        return Err("Önizlenecek satır yok.".into());
    }
    let needs_columns = rows.iter().any(|r| !r.sil);
    let columns = if needs_columns {
        Some(db::fetch_cari_columns(&cfg).await?)
    } else {
        None
    };
    db::build_sql_preview(
        &rows,
        &triggers,
        cari_tipi,
        user_id,
        son_deg_guncelle,
        columns.as_deref(),
    )
}

/// Aktarım öncesi ön kontrol: her satır için eski kod var mı, yeni kod zaten
/// mevcut mu. Aktarımı çalıştırmaz.
#[tauri::command]
async fn validate_rows(
    cfg: DbConfig,
    rows: Vec<TransferRow>,
) -> Result<Vec<db::RowValidation>, String> {
    if rows.is_empty() {
        return Ok(vec![]);
    }
    db::validate_rows(&cfg, &rows).await
}

/// F10 cari arama: verilen view'da '*' joker desteğiyle cari_kod araması yapar.
#[tauri::command]
async fn search_cari(
    cfg: DbConfig,
    view: String,
    term: String,
    limit: i32,
) -> Result<db::CariSearchResult, String> {
    let view = if view.trim().is_empty() {
        "dbo.CARI_HESAPLAR_CHOOSE_2A_1".to_string()
    } else {
        view
    };
    db::search_cari(&cfg, &view, &term, if limit <= 0 { 100000 } else { limit }).await
}

/// Listedeki trigger'lara SESSION_CONTEXT muhafızı için üretilecek ALTER'ları
/// önizler (çalıştırmaz). install=true → ekleme, false → kaldırma.
#[tauri::command]
async fn preview_trigger_guard(
    cfg: DbConfig,
    triggers: Vec<TriggerCfg>,
    key: String,
    install: bool,
) -> Result<Vec<db::GuardOutcome>, String> {
    db::prepare_trigger_guards(&cfg, &triggers, &key, install, false).await
}

/// Listedeki trigger'lara muhafızı gerçekten uygular (ALTER çalıştırır).
#[tauri::command]
async fn apply_trigger_guard(
    cfg: DbConfig,
    triggers: Vec<TriggerCfg>,
    key: String,
    install: bool,
) -> Result<Vec<db::GuardOutcome>, String> {
    db::prepare_trigger_guards(&cfg, &triggers, &key, install, true).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .on_window_event(|_window, _event| {
            // macOS geleneği: pencere X ile kapatılınca uygulama çıkmasın,
            // pencere gizlensin (uygulama Dock'ta kalır). ⌘Q ile çıkılır.
            #[cfg(target_os = "macos")]
            if let tauri::WindowEvent::CloseRequested { api, .. } = _event {
                api.prevent_close();
                let _ = _window.hide();
            }
        })
        .setup(|app| {
            app.manage(AppState::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            test_connection,
            backup_database,
            trigger_status,
            preview_transfer_sql,
            run_transfer,
            cancel_transfer,
            enable_trigger,
            search_cari,
            validate_rows,
            preview_trigger_guard,
            apply_trigger_guard,
            settings::save_settings,
            settings::load_settings,
        ])
        .build(tauri::generate_context!())
        .expect("Tauri uygulaması başlatılamadı")
        .run(|_app_handle, _event| {
            // macOS: Dock ikonuna tıklanınca gizlenmiş pencereyi geri getir.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                if let Some(w) = _app_handle.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        });
}
