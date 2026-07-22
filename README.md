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

1. Veritabanının yedeğini alın ve Mikro'yu kapatın.
2. Bağlantı bilgilerini girip **Bağlantıyı Test Et** düğmesine basın.
3. Cari kodlarını ekleyin; işaretli **Eski Kart Silinsin** seçeneği kartı
   yeniden adlandırır, işaretsiz seçenek eski kartı koruyup yeni kart oluşturur.
4. Aktarımı onaylayın ve işlem günlüğünü kontrol edin.

Her satır kendi SQL transaction'ında işlenir. Bir satırın hatası diğer satırları
engellemez. Trigger geri açılamazsa uygulama kırmızı kritik uyarı verir; SQL
Server'da `ENABLE TRIGGER ... ON ...` komutunu çalıştırmadan devam etmeyin.
