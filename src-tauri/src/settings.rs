//! Ayarların kalıcı olarak saklanması.
//!
//! - Gizli olmayan ayarlar (sunucu, veritabanı, trigger'lar, yedek yolu vs.)
//!   app config dizinine düz JSON olarak yazılır: `settings.json`.
//! - SQL parolası ASLA düz metin yazılmaz. AES-256-GCM ile, cihazda tutulan
//!   rastgele bir anahtarla (`secret.key`, yalnız sahibi okuyabilir) şifrelenip
//!   `secret.bin` dosyasına yazılır.

use std::fs;
use std::io::Write;
use std::path::PathBuf;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::db::TriggerCfg;

/// Diskte saklanan ayar kümesi. Parola burada YOKTUR (ayrı şifreli tutulur).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub server: String,
    pub database: String,
    pub auth: String,
    pub username: String,
    pub trust_cert: bool,
    pub backup_directory: String,
    pub triggers: Vec<TriggerCfg>,
    pub cari_tipi: i32,
    pub user_id: i32,
    pub son_deg_guncelle: bool,
    /// Parolayı da kaydet/kullan işareti. false ise secret.bin silinir.
    pub remember_password: bool,
}

/// Frontend ile taşınan tam yük (parola dahil).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SettingsPayload {
    #[serde(flatten)]
    pub settings: AppSettings,
    pub password: String,
}

fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("Yapılandırma dizini alınamadı: {e}"))?;
    fs::create_dir_all(&dir).map_err(|e| format!("Yapılandırma dizini oluşturulamadı: {e}"))?;
    Ok(dir)
}

fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("settings.json"))
}

fn secret_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("secret.bin"))
}

fn key_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("secret.key"))
}

/// Şifreleme anahtarını okur; yoksa 32 baytlık rastgele bir anahtar üretip
/// yalnız sahibinin okuyabileceği izinlerle kaydeder.
fn load_or_create_key(app: &tauri::AppHandle) -> Result<[u8; 32], String> {
    let path = key_path(app)?;
    if let Ok(bytes) = fs::read(&path) {
        if bytes.len() == 32 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            return Ok(key);
        }
    }
    let mut key = [0u8; 32];
    getrandom::getrandom(&mut key).map_err(|e| format!("Anahtar üretilemedi: {e}"))?;
    write_private(&path, &key)?;
    Ok(key)
}

/// Dosyayı yazar ve (Unix'te) izinlerini 0600 yapar.
fn write_private(path: &PathBuf, data: &[u8]) -> Result<(), String> {
    let mut file = fs::File::create(path).map_err(|e| format!("Dosya yazılamadı: {e}"))?;
    file.write_all(data).map_err(|e| format!("Dosya yazılamadı: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn encrypt_password(app: &tauri::AppHandle, password: &str) -> Result<(), String> {
    let key_bytes = load_or_create_key(app)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

    let mut nonce_bytes = [0u8; 12];
    getrandom::getrandom(&mut nonce_bytes).map_err(|e| format!("Nonce üretilemedi: {e}"))?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, password.as_bytes())
        .map_err(|e| format!("Parola şifrelenemedi: {e}"))?;

    // Dosya biçimi: [12 bayt nonce][ciphertext]
    let mut blob = Vec::with_capacity(12 + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(&ciphertext);
    write_private(&secret_path(app)?, &blob)
}

fn decrypt_password(app: &tauri::AppHandle) -> Result<String, String> {
    let blob = match fs::read(secret_path(app)?) {
        Ok(b) => b,
        Err(_) => return Ok(String::new()), // kayıtlı parola yok
    };
    if blob.len() < 12 + 16 {
        return Ok(String::new());
    }
    let key_bytes = load_or_create_key(app)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key_bytes));

    let (nonce_bytes, ciphertext) = blob.split_at(12);
    let plaintext = cipher
        .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
        .map_err(|_| "Parola çözülemedi (anahtar değişmiş olabilir).".to_string())?;

    String::from_utf8(plaintext).map_err(|_| "Parola geçersiz kodlama.".to_string())
}

fn clear_secret(app: &tauri::AppHandle) {
    let _ = fs::remove_file(secret_path(app).unwrap_or_default());
}

// ---------------------------------------------------------------------------
// Komutlar
// ---------------------------------------------------------------------------

/// Ayarları ve (istenirse şifreli) parolayı kaydeder.
#[tauri::command]
pub async fn save_settings(app: tauri::AppHandle, payload: SettingsPayload) -> Result<(), String> {
    let json = serde_json::to_string_pretty(&payload.settings)
        .map_err(|e| format!("Ayarlar serileştirilemedi: {e}"))?;
    fs::write(settings_path(&app)?, json).map_err(|e| format!("Ayarlar kaydedilemedi: {e}"))?;

    if payload.settings.remember_password && !payload.password.is_empty() {
        encrypt_password(&app, &payload.password)?;
    } else {
        clear_secret(&app);
    }
    Ok(())
}

/// Ayarları ve (varsa) çözülmüş parolayı yükler. Kayıt yoksa varsayılan döner.
#[tauri::command]
pub async fn load_settings(app: tauri::AppHandle) -> Result<SettingsPayload, String> {
    let settings: AppSettings = match fs::read_to_string(settings_path(&app)?) {
        Ok(s) => serde_json::from_str(&s).map_err(|e| format!("Ayarlar okunamadı: {e}"))?,
        Err(_) => return Ok(SettingsPayload::default()),
    };

    let password = if settings.remember_password {
        decrypt_password(&app).unwrap_or_default()
    } else {
        String::new()
    };

    Ok(SettingsPayload { settings, password })
}
