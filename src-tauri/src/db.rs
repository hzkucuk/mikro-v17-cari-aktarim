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

#[cfg(test)]
mod tests {
    use super::validate_qualified_ident;

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
