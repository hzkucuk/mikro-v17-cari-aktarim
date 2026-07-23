//! SQL Server bağlantısı ve Mikro cari kartı aktarım mantığı.
//!
//! Aktarımın özü iki adımdır (SQL Profiler ile Mikro'nun kendi modülünden yakalandı):
//!   1. CARI_HESAPLAR header'ı yeni koda çevrilir (rename) veya kopyalanır (INSERT).
//!   2. `dbo.msp_CariKodunuDegistir` çağrılarak 200+ tablodaki referanslar düzeltilir.
//!
//! Kendi UPDATE'lerimizi YAZMIYORUZ — Mikro'nun kendi SP'sini çağırıyoruz ki
//! sürüm farklarında tablo listesi otomatik doğru kalsın.

use serde::{Deserialize, Serialize};
use tiberius::{AuthMethod, Client, Config, SqlBrowser};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

pub type SqlClient = Client<Compat<TcpStream>>;

/// CARI_HESAPLAR tablosunun tam adı. Şema sabit: Mikro her zaman dbo kullanır.
const CARI_TABLE: &str = "dbo.CARI_HESAPLAR";

// ---------------------------------------------------------------------------
// Payload tipleri (frontend ile paylaşılan sözleşme)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DbConfig {
    /// "10.0.0.10", "10.0.0.10\\SQLEXPRESS" veya "10.0.0.10,1433"
    pub server: String,
    pub database: String,
    /// "windows" | "sql"
    pub auth: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    /// Mikro sunucuları genelde self-signed sertifika kullanır.
    #[serde(default = "default_true")]
    pub trust_cert: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct TriggerCfg {
    /// örn. "dbo.tr_Siparis_ForinsertUpdate"
    pub name: String,
    /// örn. "dbo.SIPARISLER"
    pub table: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransferRow {
    pub eski: String,
    pub yeni: String,
    /// true  -> eski kart silinsin (rename optimizasyonu)
    /// false -> eski kart kalsın (INSERT ile kopyala)
    pub sil: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RowStatus {
    pub index: usize,
    pub eski: String,
    pub yeni: String,
    /// "running" | "ok" | "error"
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferSummary {
    pub total: usize,
    pub ok: usize,
    pub failed: usize,
    /// Trigger tekrar açılabildi mi? false ise KULLANICI MÜDAHALE ETMELİ.
    pub trigger_restored: bool,
    pub trigger_message: String,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    pub path: String,
    pub message: String,
}

/// `dbo.CARI_AKTARIM_LOG` tablosuna yazılan denetim kaydı.
pub struct AuditEntry<'a> {
    pub client_machine: &'a str,
    pub client_user: &'a str,
    pub eski: &'a str,
    pub yeni: &'a str,
    pub cari_tipi: i32,
    pub sil: bool,
    pub result: &'a str,
    pub message: &'a str,
}

// ---------------------------------------------------------------------------
// Kimlik (identifier) doğrulama
// ---------------------------------------------------------------------------

/// DDL'de (DISABLE/ENABLE TRIGGER) parametre kullanılamaz, isim string olarak
/// gömülmek zorunda. Bu yüzden SQL injection'a karşı katı doğrulama yapıyoruz:
/// sadece `sema.nesne` biçimi, her parça harf/rakam/_ /$ /# karakterlerinden
/// oluşacak ve harf veya _ ile başlayacak. Köşeli parantezler yalnızca doğru
/// eşleşmiş hâlde kabul edilir; çıktıda tekrar güvenli biçimde eklenir.
pub fn validate_qualified_ident(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("Boş nesne adı.".into());
    }

    let parts: Vec<&str> = raw.split('.').collect();
    if parts.len() > 2 {
        return Err(format!(
            "Geçersiz nesne adı '{raw}'. En fazla 'sema.nesne' biçimi desteklenir."
        ));
    }

    let mut out = Vec::with_capacity(parts.len());
    for part in parts {
        let raw_part = part.trim();
        let has_open = raw_part.starts_with('[');
        let has_close = raw_part.ends_with(']');
        if has_open != has_close {
            return Err(format!(
                "Geçersiz nesne adı '{raw}': köşeli parantezler eşleşmiyor."
            ));
        }
        let p = if has_open {
            &raw_part[1..raw_part.len() - 1]
        } else {
            raw_part
        };
        if p.is_empty() {
            return Err(format!("Geçersiz nesne adı '{raw}': boş parça."));
        }
        let mut chars = p.chars();
        let first = chars.next().unwrap();
        if !(first.is_ascii_alphabetic() || first == '_') {
            return Err(format!(
                "Geçersiz nesne adı '{raw}': '{p}' harf veya _ ile başlamalı."
            ));
        }
        if !p
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '$' | '#' | '@'))
        {
            return Err(format!(
                "Geçersiz nesne adı '{raw}': '{p}' izin verilmeyen karakter içeriyor."
            ));
        }
        out.push(format!("[{p}]"));
    }

    Ok(out.join("."))
}

/// Veritabanı adı DDL içinde parametrelenemez. Köşeli parantez kaçışı SQL
/// Server'ın identifier kuralına uygundur; kontrol karakterleri ise kabul
/// edilmez. Böylece nokta/boşluk içeren geçerli veritabanı adları da çalışır.
fn quote_database_ident(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("Veritabanı adı boş.".into());
    }
    if name.chars().any(char::is_control) {
        return Err("Veritabanı adı kontrol karakteri içeremez.".into());
    }
    Ok(format!("[{}]", name.replace(']', "]]")))
}

// ---------------------------------------------------------------------------
// Bağlantı
// ---------------------------------------------------------------------------

pub async fn connect(cfg: &DbConfig) -> Result<SqlClient, String> {
    let mut config = Config::new();

    // "host\instance" ve "host,port" biçimlerini ayrıştır.
    let server = cfg.server.trim();
    if server.is_empty() {
        return Err("Sunucu adı boş.".into());
    }
    if cfg.database.trim().is_empty() {
        return Err("Veritabanı adı boş.".into());
    }
    if let Some((host, instance)) = server.split_once('\\') {
        config.host(host.trim());
        config.instance_name(instance.trim());
    } else if let Some((host, port)) = server.split_once(',') {
        config.host(host.trim());
        let port: u16 = port
            .trim()
            .parse()
            .map_err(|_| format!("Geçersiz port: '{port}'"))?;
        config.port(port);
    } else {
        config.host(server);
        config.port(1433);
    }

    config.database(cfg.database.trim());

    if cfg.trust_cert {
        config.trust_cert();
    }

    match cfg.auth.trim().to_ascii_lowercase().as_str() {
        "sql" => {
            let user = cfg
                .username
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("SQL Auth seçildi ama kullanıcı adı boş.")?;
            let pass = cfg.password.as_deref().unwrap_or("");
            config.authentication(AuthMethod::sql_server(user, pass));
        }
        "windows" => {
            #[cfg(windows)]
            {
                config.authentication(AuthMethod::Integrated);
            }
            #[cfg(not(windows))]
            {
                return Err(
                    "Windows Integrated Auth yalnızca Windows üzerinde desteklenir. \
                     Bu platformda SQL Server Auth kullanın."
                        .into(),
                );
            }
        }
        other => return Err(format!("Bilinmeyen kimlik doğrulama türü: '{other}'.")),
    }

    // connect_named hem düz host:port hem de adlandırılmış örnek (SQL Browser)
    // senaryosunu kapsar.
    let tcp = TcpStream::connect_named(&config)
        .await
        .map_err(|e| format!("Sunucuya bağlanılamadı ({}): {e}", cfg.server))?;
    tcp.set_nodelay(true)
        .map_err(|e| format!("TCP ayarı yapılamadı: {e}"))?;

    Client::connect(config, tcp.compat_write())
        .await
        .map_err(|e| format!("SQL Server oturumu açılamadı: {e}"))
}

/// Bağlantıyı doğrular ve sunucu/veritabanı bilgisini döner.
pub async fn probe(client: &mut SqlClient) -> Result<String, String> {
    let rows = client
        .query(
            "SELECT @@VERSION, DB_NAME(), SUSER_SNAME(), \
             CONVERT(nvarchar(50), SERVERPROPERTY('ProductVersion'))",
            &[],
        )
        .await
        .map_err(|e| format!("Sorgu başarısız: {e}"))?
        .into_first_result()
        .await
        .map_err(|e| format!("Sonuç okunamadı: {e}"))?;

    let row = rows.into_iter().next().ok_or("Sunucudan yanıt gelmedi.")?;

    let version: &str = row.get(0).unwrap_or("");
    let db: &str = row.get(1).unwrap_or("");
    let login: &str = row.get(2).unwrap_or("");
    let prod: &str = row.get(3).unwrap_or("");

    let first_line = version.lines().next().unwrap_or(version).trim();

    Ok(format!(
        "Bağlantı başarılı.\nVeritabanı: {db}\nOturum: {login}\nSürüm: {prod}\n{first_line}"
    ))
}

/// `msp_CariKodunuDegistir` bu veritabanında var mı? Yoksa aktarım anlamsız.
pub async fn check_sp_exists(client: &mut SqlClient) -> Result<bool, String> {
    let rows = client
        .query(
            "SELECT COUNT(*) FROM sys.objects \
             WHERE type IN ('P','PC') AND name = 'msp_CariKodunuDegistir'",
            &[],
        )
        .await
        .map_err(|e| format!("SP kontrolü başarısız: {e}"))?
        .into_first_result()
        .await
        .map_err(|e| format!("SP kontrol sonucu okunamadı: {e}"))?;

    let cnt: i32 = rows
        .into_iter()
        .next()
        .and_then(|r| r.get::<i32, _>(0))
        .unwrap_or(0);

    Ok(cnt > 0)
}

/// Denetim tablosunu ilk kullanımda oluşturur. Aktarım kaydı, Mikro'nun
/// tablolarını değiştiren her satırın izini SQL Server'da kalıcı tutar.
pub async fn ensure_audit_log_table(client: &mut SqlClient) -> Result<(), String> {
    const SQL: &str = "
        IF OBJECT_ID(N'dbo.CARI_AKTARIM_LOG', N'U') IS NULL
        BEGIN
          CREATE TABLE dbo.CARI_AKTARIM_LOG (
            log_id bigint IDENTITY(1,1) NOT NULL PRIMARY KEY,
            log_date datetime2(0) NOT NULL CONSTRAINT DF_CARI_AKTARIM_LOG_date DEFAULT SYSDATETIME(),
            client_machine nvarchar(128) NOT NULL,
            client_user nvarchar(256) NOT NULL,
            sql_login nvarchar(256) NOT NULL CONSTRAINT DF_CARI_AKTARIM_LOG_login DEFAULT SUSER_SNAME(),
            eski_cari_kod nvarchar(255) NOT NULL,
            yeni_cari_kod nvarchar(255) NOT NULL,
            cari_tipi int NOT NULL,
            eski_kart_silinsin bit NOT NULL,
            result nvarchar(16) NOT NULL,
            message nvarchar(max) NULL
          );
        END";
    exec_simple(client, SQL)
        .await
        .map_err(|e| format!("Denetim tablosu oluşturulamadı: {e}"))
}

pub async fn ensure_trigger_guard_table(client: &mut SqlClient) -> Result<(), String> {
    const SQL: &str = "
        IF OBJECT_ID(N'dbo.CARI_AKTARIM_TRIGGER_GUARD', N'U') IS NULL
        BEGIN
          CREATE TABLE dbo.CARI_AKTARIM_TRIGGER_GUARD (
            guard_id bigint IDENTITY(1,1) NOT NULL PRIMARY KEY,
            trigger_name nvarchar(260) NOT NULL,
            table_name nvarchar(260) NOT NULL,
            client_machine nvarchar(128) NOT NULL,
            client_user nvarchar(256) NOT NULL,
            disabled_at datetime2(0) NOT NULL CONSTRAINT DF_CARI_AKTARIM_TRIGGER_GUARD_disabled DEFAULT SYSDATETIME(),
            restored_at datetime2(0) NULL,
            restore_note nvarchar(1000) NULL
          );
        END";
    exec_simple(client, SQL)
        .await
        .map_err(|e| format!("Trigger koruma tablosu oluşturulamadı: {e}"))
}

pub async fn trigger_is_disabled(client: &mut SqlClient, trigger: &str) -> Result<bool, String> {
    let rows = client
        .query(
            "SELECT is_disabled FROM sys.triggers WHERE object_id = OBJECT_ID(@P1)",
            &[&trigger],
        )
        .await
        .map_err(|e| format!("Trigger durumu okunamadı: {e}"))?
        .into_first_result()
        .await
        .map_err(|e| format!("Trigger durumu sonucu okunamadı: {e}"))?;
    rows.into_iter()
        .next()
        .and_then(|r| r.get::<bool, _>(0))
        .ok_or_else(|| format!("Trigger bulunamadı: {trigger}"))
}

pub async fn register_trigger_guard(
    client: &mut SqlClient,
    trigger: &str,
    table: &str,
    machine: &str,
    user: &str,
) -> Result<i64, String> {
    let rows = client
        .query(
            "INSERT INTO dbo.CARI_AKTARIM_TRIGGER_GUARD \
             (trigger_name, table_name, client_machine, client_user) \
             OUTPUT INSERTED.guard_id VALUES (@P1, @P2, @P3, @P4)",
            &[&trigger, &table, &machine, &user],
        )
        .await
        .map_err(|e| format!("Trigger koruma kaydı oluşturulamadı: {e}"))?
        .into_first_result()
        .await
        .map_err(|e| format!("Trigger koruma kaydı okunamadı: {e}"))?;
    rows.into_iter()
        .next()
        .and_then(|r| r.get::<i64, _>(0))
        .ok_or("Trigger koruma kaydı kimliği alınamadı.".into())
}

pub async fn resolve_trigger_guard(
    client: &mut SqlClient,
    guard_id: i64,
    note: &str,
) -> Result<(), String> {
    client
        .execute(
            "UPDATE dbo.CARI_AKTARIM_TRIGGER_GUARD \
             SET restored_at = SYSDATETIME(), restore_note = @P1 WHERE guard_id = @P2",
            &[&note, &guard_id],
        )
        .await
        .map_err(|e| format!("Trigger koruma kaydı kapatılamadı: {e}"))?;
    Ok(())
}

pub async fn pending_trigger_guard(
    client: &mut SqlClient,
    trigger: &str,
    table: &str,
) -> Result<Option<i64>, String> {
    let rows = client
        .query(
            "SELECT TOP 1 guard_id FROM dbo.CARI_AKTARIM_TRIGGER_GUARD \
             WHERE trigger_name = @P1 AND table_name = @P2 AND restored_at IS NULL \
             ORDER BY guard_id DESC",
            &[&trigger, &table],
        )
        .await
        .map_err(|e| format!("Bekleyen trigger koruma kaydı okunamadı: {e}"))?
        .into_first_result()
        .await
        .map_err(|e| format!("Bekleyen trigger koruma sonucu okunamadı: {e}"))?;
    Ok(rows.into_iter().next().and_then(|r| r.get::<i64, _>(0)))
}

/// Başarılı ve başarısız her satırı transaction sonrasında kaydeder.
pub async fn write_audit_log(client: &mut SqlClient, entry: AuditEntry<'_>) -> Result<(), String> {
    let sil: i32 = i32::from(entry.sil);
    client
        .execute(
            "INSERT INTO dbo.CARI_AKTARIM_LOG \
             (client_machine, client_user, eski_cari_kod, yeni_cari_kod, cari_tipi, eski_kart_silinsin, result, message) \
             VALUES (@P1, @P2, @P3, @P4, @P5, @P6, @P7, @P8)",
            &[
                &entry.client_machine,
                &entry.client_user,
                &entry.eski,
                &entry.yeni,
                &entry.cari_tipi,
                &sil,
                &entry.result,
                &entry.message,
            ],
        )
        .await
        .map_err(|e| format!("Denetim kaydı yazılamadı: {e}"))?;
    Ok(())
}

/// SQL Server'ın GÖRDÜĞÜ klasöre, aktarım öncesi COPY_ONLY tam yedek alır.
/// COPY_ONLY mevcut diferansiyel/log yedekleme zincirini etkilemez.
pub async fn backup_database(
    cfg: &DbConfig,
    backup_directory: &str,
) -> Result<BackupResult, String> {
    let directory = backup_directory.trim().trim_end_matches(['\\', '/']);
    if directory.is_empty() {
        return Err(
            "Yedek klasörü boş. SQL Server'ın erişebildiği bir Windows/UNC klasörü girin.".into(),
        );
    }

    let database = quote_database_ident(&cfg.database)?;
    // Dosya adı, bağlantıdan gelen DB adını güvenle dosya adına dönüştürür.
    let safe_db_name: String = cfg
        .database
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("Sistem saati okunamadı: {e}"))?
        .as_secs();
    let path = format!("{directory}\\{safe_db_name}_cari_aktarim_oncesi_{stamp}.bak");

    let mut client = connect(cfg).await?;
    let sql = format!(
        "BACKUP DATABASE {database} TO DISK = @P1 \
         WITH COPY_ONLY, COMPRESSION, CHECKSUM, STATS = 10"
    );
    exec_simple_with_param(&mut client, &sql, &path)
        .await
        .map_err(|e| {
            format!(
                "Yedek alınamadı: {e}. SQL Server hizmet hesabının '{directory}' klasörüne yazma yetkisini kontrol edin."
            )
        })?;

    Ok(BackupResult {
        path: path.clone(),
        message: format!("COPY_ONLY tam yedek başarıyla alındı: {path}"),
    })
}

// ---------------------------------------------------------------------------
// Yardımcılar
// ---------------------------------------------------------------------------

async fn exec_simple(client: &mut SqlClient, sql: &str) -> Result<(), String> {
    client
        .simple_query(sql)
        .await
        .map_err(|e| format!("{e}"))?
        .into_results()
        .await
        .map_err(|e| format!("{e}"))?;
    Ok(())
}

async fn exec_simple_with_param(
    client: &mut SqlClient,
    sql: &str,
    value: &str,
) -> Result<(), String> {
    client
        .query(sql, &[&value])
        .await
        .map_err(|e| format!("{e}"))?
        .into_results()
        .await
        .map_err(|e| format!("{e}"))?;
    Ok(())
}

async fn scalar_count(client: &mut SqlClient, sql: &str, code: &str) -> Result<i32, String> {
    let rows = client
        .query(sql, &[&code])
        .await
        .map_err(|e| format!("{e}"))?
        .into_first_result()
        .await
        .map_err(|e| format!("{e}"))?;

    Ok(rows
        .into_iter()
        .next()
        .and_then(|r| r.get::<i32, _>(0))
        .unwrap_or(0))
}

/// CARI_HESAPLAR'ın kopyalanabilir kolonlarını ORDINAL_POSITION sırasıyla getirir.
/// IDENTITY, COMPUTED ve TIMESTAMP/ROWVERSION kolonları INSERT'e dahil edilemez.
async fn cari_columns(client: &mut SqlClient) -> Result<Vec<String>, String> {
    const SQL: &str = "
        SELECT c.COLUMN_NAME
        FROM INFORMATION_SCHEMA.COLUMNS c
        WHERE c.TABLE_SCHEMA = 'dbo'
          AND c.TABLE_NAME   = 'CARI_HESAPLAR'
          AND c.DATA_TYPE   <> 'timestamp'
          AND COLUMNPROPERTY(OBJECT_ID('dbo.CARI_HESAPLAR'), c.COLUMN_NAME, 'IsIdentity') = 0
          AND COLUMNPROPERTY(OBJECT_ID('dbo.CARI_HESAPLAR'), c.COLUMN_NAME, 'IsComputed') = 0
        ORDER BY c.ORDINAL_POSITION";

    let rows = client
        .query(SQL, &[])
        .await
        .map_err(|e| format!("Kolon listesi alınamadı: {e}"))?
        .into_first_result()
        .await
        .map_err(|e| format!("Kolon listesi okunamadı: {e}"))?;

    let cols: Vec<String> = rows
        .into_iter()
        .filter_map(|r| r.get::<&str, _>(0).map(str::to_owned))
        .collect();

    if cols.is_empty() {
        return Err("CARI_HESAPLAR tablosu bulunamadı veya kolonu yok.".into());
    }
    Ok(cols)
}

// ---------------------------------------------------------------------------
// Trigger yönetimi
// ---------------------------------------------------------------------------

pub async fn set_trigger(
    client: &mut SqlClient,
    trigger: &str,
    table: &str,
    enable: bool,
) -> Result<(), String> {
    let verb = if enable { "ENABLE" } else { "DISABLE" };
    let sql = format!("{verb} TRIGGER {trigger} ON {table}");
    exec_simple(client, &sql)
        .await
        .map_err(|e| format!("{verb} TRIGGER başarısız: {e}"))
}

// ---------------------------------------------------------------------------
// Tek satır aktarımı
// ---------------------------------------------------------------------------

/// Bir cari kodunun aktarımını kendi transaction'ı içinde yapar.
/// Hata durumunda ROLLBACK eder ve hatayı döner; çağıran döngü devam edebilir.
pub async fn transfer_one(
    client: &mut SqlClient,
    row: &TransferRow,
    cari_tipi: i32,
    user_id: i32,
    son_deg_guncelle: bool,
    client_machine: &str,
    client_user: &str,
) -> Result<String, String> {
    let eski = row.eski.trim();
    let yeni = row.yeni.trim();

    if eski.is_empty() || yeni.is_empty() {
        return Err("Eski veya yeni kod boş.".into());
    }
    if eski == yeni {
        return Err("Eski ve yeni kod aynı.".into());
    }

    exec_simple(client, "BEGIN TRANSACTION")
        .await
        .map_err(|e| format!("BEGIN TRAN başarısız: {e}"))?;

    let result = transfer_one_inner(
        client,
        eski,
        yeni,
        row.sil,
        cari_tipi,
        user_id,
        son_deg_guncelle,
    )
    .await;

    match result {
        Ok(msg) => {
            // Başarılı aktarım ve denetim kaydı aynı transaction içinde
            // kalır; log yazılamazsa aktarım da geri alınır.
            if let Err(audit_error) = write_audit_log(
                client,
                AuditEntry {
                    client_machine,
                    client_user,
                    eski,
                    yeni,
                    cari_tipi,
                    sil: row.sil,
                    result: "OK",
                    message: &msg,
                },
            )
            .await
            {
                let _ = exec_simple(client, "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION").await;
                return Err(format!(
                    "Aktarım denetim kaydı yazılamadığı için geri alındı: {audit_error}"
                ));
            }
            exec_simple(client, "COMMIT TRANSACTION")
                .await
                .map_err(|e| format!("COMMIT başarısız: {e}"))?;
            Ok(msg)
        }
        Err(e) => {
            // ROLLBACK'in kendisi de patlarsa asıl hatayı kaybetmeyelim.
            if let Err(rb) = exec_simple(client, "IF @@TRANCOUNT > 0 ROLLBACK TRANSACTION").await {
                return Err(format!("{e} | ROLLBACK da başarısız: {rb}"));
            }
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn transfer_one_inner(
    client: &mut SqlClient,
    eski: &str,
    yeni: &str,
    sil: bool,
    cari_tipi: i32,
    user_id: i32,
    son_deg_guncelle: bool,
) -> Result<String, String> {
    // --- Ön kontroller -----------------------------------------------------
    let eski_var = scalar_count(
        client,
        "SELECT COUNT(*) FROM dbo.CARI_HESAPLAR WHERE cari_kod = @P1",
        eski,
    )
    .await?;
    if eski_var == 0 {
        return Err(format!("Eski kod bulunamadı: '{eski}'"));
    }

    let yeni_var = scalar_count(
        client,
        "SELECT COUNT(*) FROM dbo.CARI_HESAPLAR WHERE cari_kod = @P1",
        yeni,
    )
    .await?;
    if yeni_var > 0 {
        return Err(format!("Yeni kod zaten mevcut: '{yeni}'"));
    }

    // --- 1. adım: header ---------------------------------------------------
    let step1 = if sil {
        // Kopyala + sil == yeniden adlandır. Mikro modülü de böyle yapıyor.
        let res = client
            .execute(
                "UPDATE dbo.CARI_HESAPLAR \
                 SET cari_kod = @P1, cari_lastup_date = GETDATE(), cari_lastup_user = @P2 \
                 WHERE cari_kod = @P3",
                &[&yeni, &user_id, &eski],
            )
            .await
            .map_err(|e| format!("Kart yeniden adlandırılamadı: {e}"))?;

        let affected: u64 = res.rows_affected().iter().sum();
        if affected != 1 {
            return Err(format!(
                "Beklenmeyen güncelleme sayısı ({affected}). 1 satır bekleniyordu."
            ));
        }
        "kart yeniden adlandırıldı"
    } else {
        // Eski kart kalsın: kolonları dinamik okuyup birebir kopyala,
        // yalnızca cari_kod alanını yeni koda çevir.
        let cols = cari_columns(client).await?;

        let col_list = cols
            .iter()
            .map(|c| format!("[{c}]"))
            .collect::<Vec<_>>()
            .join(", ");

        let select_list = cols
            .iter()
            .map(|c| {
                if c.eq_ignore_ascii_case("cari_kod") {
                    "@P1".to_string()
                } else {
                    format!("[{c}]")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");

        let sql = format!(
            "INSERT INTO {CARI_TABLE} ({col_list}) \
             SELECT {select_list} FROM {CARI_TABLE} WHERE cari_kod = @P2"
        );

        let res = client
            .execute(&sql, &[&yeni, &eski])
            .await
            .map_err(|e| format!("Yeni kart oluşturulamadı: {e}"))?;

        let affected: u64 = res.rows_affected().iter().sum();
        if affected != 1 {
            return Err(format!(
                "Beklenmeyen ekleme sayısı ({affected}). 1 satır bekleniyordu."
            ));
        }
        "yeni kart oluşturuldu"
    };

    // --- 2. adım: referansları düzelt --------------------------------------
    // msp_CariKodunuDegistir SADECE referansları günceller, ana kartı değil.
    // Bu yüzden header işlemi mutlaka bundan ÖNCE yapılmalıdır.
    let flag: i32 = if son_deg_guncelle { 1 } else { 0 };

    client
        .execute(
            "EXEC dbo.msp_CariKodunuDegistir \
                 @CariTipi = @P1, \
                 @EskiCariKodu = @P2, \
                 @YeniCariKodu = @P3, \
                 @Kart_aktarim_son_deg_bilgileri_guncelle_fl = @P4",
            &[&cari_tipi, &eski, &yeni, &flag],
        )
        .await
        .map_err(|e| format!("msp_CariKodunuDegistir başarısız: {e}"))?;

    Ok(format!("{step1}, referanslar aktarıldı"))
}

// ---------------------------------------------------------------------------
// SQL önizlemesi (aktarım öncesi onay için)
// ---------------------------------------------------------------------------

/// Bir değeri SQL string literal'ine çevirir: `N'...'`, tek tırnaklar kaçırılır.
/// Yalnızca önizleme metni içindir; asıl yürütme parametreli sorgu kullanır.
fn sql_literal(s: &str) -> String {
    format!("N'{}'", s.replace('\'', "''"))
}

/// Kopyalama önizlemesi için CARI_HESAPLAR kolonlarını canlı bağlantıdan alır.
pub async fn fetch_cari_columns(cfg: &DbConfig) -> Result<Vec<String>, String> {
    let mut client = connect(cfg).await?;
    cari_columns(&mut client).await
}

/// Aktarımda ÇALIŞTIRILACAK SQL'in insan-okur önizlemesini üretir. Yürütme
/// akışıyla birebir aynıdır (trigger disable → satır başına BEGIN/UPDATE|INSERT/
/// EXEC/COMMIT → trigger enable). Kopyalama (sil=false) satırları için gerçek
/// kolon listesi `columns` ile verilmelidir; verilmezse yer tutucu yazılır.
pub fn build_sql_preview(
    rows: &[TransferRow],
    triggers: &[TriggerCfg],
    cari_tipi: i32,
    user_id: i32,
    son_deg_guncelle: bool,
    columns: Option<&[String]>,
) -> Result<String, String> {
    let flag: i32 = if son_deg_guncelle { 1 } else { 0 };
    let sep = "-- ================================================================\n";
    let mut out = String::new();

    // Trigger'ları yürütmeyle aynı katı doğrulamadan geçir.
    let mut trig_idents = Vec::new();
    for t in triggers {
        if t.name.trim().is_empty() && t.table.trim().is_empty() {
            continue;
        }
        trig_idents.push((
            validate_qualified_ident(&t.name)?,
            validate_qualified_ident(&t.table)?,
        ));
    }

    out.push_str(sep);
    out.push_str("-- 1) TRIGGER'LAR DEVRE DIŞI (aktarım başında, tek oturumda)\n");
    out.push_str(sep);
    if trig_idents.is_empty() {
        out.push_str("-- (Trigger yönetimi yok — tanımlı trigger girilmedi)\n");
    } else {
        for (n, t) in &trig_idents {
            out.push_str(&format!("DISABLE TRIGGER {n} ON {t};\n"));
        }
    }
    out.push('\n');

    for (i, row) in rows.iter().enumerate() {
        let eski = row.eski.trim();
        let yeni = row.yeni.trim();
        let mode = if row.sil {
            "eski kart SİLİNECEK (yeniden adlandırma)"
        } else {
            "eski kart KORUNACAK (kopyalama)"
        };
        out.push_str(sep);
        out.push_str(&format!("-- SATIR {}:  {eski}  →  {yeni}   [{mode}]\n", i + 1));
        out.push_str(sep);
        out.push_str("BEGIN TRANSACTION;\n");

        if row.sil {
            out.push_str(&format!(
                "UPDATE dbo.CARI_HESAPLAR\n    \
                 SET cari_kod = {}, cari_lastup_date = GETDATE(), cari_lastup_user = {user_id}\n    \
                 WHERE cari_kod = {};\n",
                sql_literal(yeni),
                sql_literal(eski),
            ));
        } else {
            match columns {
                Some(cols) if !cols.is_empty() => {
                    let col_list = cols
                        .iter()
                        .map(|c| format!("[{c}]"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let select_list = cols
                        .iter()
                        .map(|c| {
                            if c.eq_ignore_ascii_case("cari_kod") {
                                sql_literal(yeni)
                            } else {
                                format!("[{c}]")
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    out.push_str(&format!(
                        "INSERT INTO dbo.CARI_HESAPLAR ({col_list})\n    \
                         SELECT {select_list}\n    \
                         FROM dbo.CARI_HESAPLAR WHERE cari_kod = {};\n",
                        sql_literal(eski),
                    ));
                }
                _ => {
                    out.push_str(&format!(
                        "INSERT INTO dbo.CARI_HESAPLAR ( <identity hariç tüm kolonlar> )\n    \
                         SELECT < ..., cari_kod = {}, ... >\n    \
                         FROM dbo.CARI_HESAPLAR WHERE cari_kod = {};\n    \
                         -- Kolon listesi çalışma anında INFORMATION_SCHEMA'dan çözülür.\n",
                        sql_literal(yeni),
                        sql_literal(eski),
                    ));
                }
            }
        }

        out.push_str(&format!(
            "EXEC dbo.msp_CariKodunuDegistir\n    \
             @CariTipi = {cari_tipi},\n    \
             @EskiCariKodu = {},\n    \
             @YeniCariKodu = {},\n    \
             @Kart_aktarim_son_deg_bilgileri_guncelle_fl = {flag};\n",
            sql_literal(eski),
            sql_literal(yeni),
        ));
        out.push_str("COMMIT TRANSACTION;\n\n");
    }

    out.push_str(sep);
    out.push_str("-- 2) TRIGGER'LAR TEKRAR ETKİN (aktarım sonunda, HER KOŞULDA)\n");
    out.push_str(sep);
    if trig_idents.is_empty() {
        out.push_str("-- (Trigger yönetimi yok)\n");
    } else {
        for (n, t) in &trig_idents {
            out.push_str(&format!("ENABLE TRIGGER {n} ON {t};\n"));
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{build_sql_preview, validate_qualified_ident, TransferRow, TriggerCfg};

    #[test]
    fn onizleme_ciktisi() {
        let rows = vec![
            TransferRow { eski: "120.1.İNT.HB.1156".into(), yeni: "ESK-120.1.İNT.HB.1156".into(), sil: true },
            TransferRow { eski: "120.1.O'BRIEN".into(), yeni: "120.1.ARSIV.OBRIEN".into(), sil: false },
        ];
        let triggers = vec![TriggerCfg { name: "dbo.tr_Siparis_ForinsertUpdate".into(), table: "dbo.SIPARISLER".into() }];
        let cols = vec!["cari_Guid".to_string(), "cari_kod".to_string(), "cari_unvan1".to_string()];
        let sql = build_sql_preview(&rows, &triggers, 0, 1, false, Some(&cols)).unwrap();
        println!("\n------- ÖNİZLEME BAŞLANGIÇ -------\n{sql}\n------- ÖNİZLEME BİTİŞ -------\n");
        // Kritik güvenlik: tek tırnak kaçışı (O'BRIEN -> O''BRIEN)
        assert!(sql.contains("N'120.1.O''BRIEN'"));
        // Rename UPDATE ve EXEC mevcut
        assert!(sql.contains("UPDATE dbo.CARI_HESAPLAR"));
        assert!(sql.contains("EXEC dbo.msp_CariKodunuDegistir"));
        // Kopyalama INSERT'inde cari_kod yeni koda çevrilmiş
        assert!(sql.contains("SELECT [cari_Guid], N'120.1.ARSIV.OBRIEN', [cari_unvan1]"));
        // Trigger disable/enable
        assert!(sql.contains("DISABLE TRIGGER [dbo].[tr_Siparis_ForinsertUpdate] ON [dbo].[SIPARISLER]"));
        assert!(sql.contains("ENABLE TRIGGER [dbo].[tr_Siparis_ForinsertUpdate] ON [dbo].[SIPARISLER]"));
    }

    #[test]
    fn kabul_edilen_isimler() {
        assert_eq!(
            validate_qualified_ident("dbo.tr_Siparis_ForinsertUpdate").unwrap(),
            "[dbo].[tr_Siparis_ForinsertUpdate]"
        );
        assert_eq!(
            validate_qualified_ident("SIPARISLER").unwrap(),
            "[SIPARISLER]"
        );
        assert_eq!(
            validate_qualified_ident("[dbo].[SIPARISLER]").unwrap(),
            "[dbo].[SIPARISLER]"
        );
    }

    #[test]
    fn injection_denemeleri_reddedilir() {
        assert!(validate_qualified_ident("SIPARISLER; DROP TABLE X--").is_err());
        assert!(validate_qualified_ident("a.b.c").is_err());
        assert!(validate_qualified_ident("").is_err());
        assert!(validate_qualified_ident("1tablo").is_err());
        assert!(validate_qualified_ident("tablo]").is_err());
        assert!(validate_qualified_ident("[tablo").is_err());
    }
}
