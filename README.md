# Mikro V17 Cari Kartı Aktarma

Mikro'nun `dbo.msp_CariKodunuDegistir` prosedürünü kullanarak cari kart
referanslarını aktarır. Aktarım boyunca belirtilen SQL Server trigger'ını kapatır
ve iş bittiğinde tekrar açar.

## Gereksinimler

- Windows 10/11
- Tauri 2.11 (proje Tauri v1 kullanmaz)
- Rust (MSVC toolchain): <https://rustup.rs>
- WebView2 Runtime (Windows 11'de yerleşiktir)
- Tauri CLI: `cargo install tauri-cli --version "^2" --locked`
- SQL Server'a erişim ve trigger'ı kapatıp açma yetkisi

Node/npm gerekmez; arayüz `dist/` altında hazır statik dosyadır.

## Otomatik güncelleme

Uygulama, GitHub Releases altındaki imzalı NSIS güncelleme paketlerini kontrol
eder. CI, paketleri `TAURI_SIGNING_PRIVATE_KEY` GitHub secret'ı ile imzalar;
özel anahtar hiçbir zaman repoya eklenmez. Yeni sürüm yayınlanırken normal
kurulum dosyasına ek olarak imzalı `.nsis.zip`, `.sig` ve `latest.json` dosyası
aynı GitHub Release'e yüklenmelidir.

## Derleme

`build.bat` dosyasına çift tıklayın. Çıktı, `src-tauri/target/release/bundle/`
altında MSI ve NSIS kurulum paketi olarak oluşur.

GitHub Actions, yalnızca `windows-latest` üzerinde MSI ve NSIS kurulum paketleri
derler; tamamlandığında **Actions > Windows package > Artifacts** içinden
`mikro-cari-aktarim-windows` indirilebilir.

İkonları yeniden üretmek gerekirse proje kökünde:

```bat
python tools\make_icon.py
```

## Kullanım güvenliği

1. Mikro'yu kapatın. SQL Server'ın yazabildiği bir yedek klasörü belirleyin
   (sunucudaki klasör veya UNC paylaşımı).
2. Bağlantı bilgilerini girip **Bağlantıyı Test Et**, ardından **Önce Yedek Al**
   düğmesine basın. Uygulama, yedek tamamlanmadan aktarımı açmaz. Yedek
   `COPY_ONLY` tam yedektir; mevcut SQL Server yedekleme zincirini etkilemez.
3. Cari kodlarını elle ekleyin veya **CSV İçeri Aktar** ile yükleyin. CSV'nin
   ilk iki sütunu eski ve yeni cari kodudur; isteğe bağlı üçüncü sütundaki
   `0`, `Hayır` veya `Kalsın` değeri eski kartın korunacağını belirtir.
4. İşaretli **Eski Kart Silinsin** seçeneği kartı
   yeniden adlandırır, işaretsiz seçenek eski kartı koruyup yeni kart oluşturur.
5. Aktarımı onaylayın ve işlem günlüğünü kontrol edin.

Birden fazla trigger gerekiyorsa **+ Trigger Ekle** ile her trigger için adı ve
bağlı olduğu tabloyu girin. Aktarım başlamadan tümü kapatılır; işlem sonunda
ters sırayla geri açılır.

Her satır kendi SQL transaction'ında işlenir. Bir satırın hatası diğer satırları
engellemez. Trigger geri açılamazsa uygulama kırmızı kritik uyarı verir; SQL
Server'da `ENABLE TRIGGER ... ON ...` komutunu çalıştırmadan devam etmeyin.
