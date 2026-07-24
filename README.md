# Mikro V17 Cari Kartı Aktarma

Mikro'nun `dbo.msp_CariKodunuDegistir` prosedürünü kullanarak cari kart
referanslarını aktarır. Aktarım boyunca belirtilen SQL Server trigger'ını kapatır
ve iş bittiğinde tekrar açar.

## Gereksinimler

- Windows 10/11
- Tauri 2.11 (proje Tauri v1 kullanmaz)
- Rust (MSVC toolchain): <https://rustup.rs>
- Node.js 22 LTS ve npm (SvelteKit arayüzünü derlemek için)
- WebView2 Runtime (Windows 11'de yerleşiktir)
- Tauri CLI: `cargo install tauri-cli --version "^2" --locked`
- SQL Server'a erişim ve trigger'ı kapatıp açma yetkisi

Arayüz SvelteKit ile `frontend/` altında tutulur. Tauri derlemesi öncesinde
`npm --prefix frontend run build` otomatik çalışır.

## Kurulum

Kurulum dosyaları [GitHub Releases](https://github.com/hzkucuk/mikro-v17-cari-aktarim/releases)
sayfasındadır. Uygulama **kod imzalı değildir**; işletim sistemleri bu yüzden ilk
açılışta uyarı gösterir. Aşağıdaki adımlar bunu çözer.

### Windows

1. `Mikro Cari Aktarim_<sürüm>_x64-setup.exe` dosyasını indirip çalıştırın.
2. SmartScreen "Bilinmeyen yayımcı" uyarısı verirse: **Ek bilgi → Yine de çalıştır**.

Kurulum **yalnızca kuran kullanıcı** içindir (`%LOCALAPPDATA%`); yönetici hakkı
gerekmez ve diğer kullanıcıların masaüstüne/başlat menüsüne kısayol eklenmez.

### macOS (Apple Silicon)

macOS, tarayıcıyla indirilen imzasız uygulamaları karantinaya alır ve yanıltıcı
biçimde **"… hasar görmüş olduğu için açılamıyor"** der. Uygulama hasarlı
değildir; karantina işaretinin kaldırılması yeterlidir:

1. `Mikro Cari Aktarim_<sürüm>_aarch64.dmg` dosyasını açın.
2. `Mikro Cari Aktarim.app`'i **Applications** klasörüne sürükleyin.
3. Disk görüntüsünü çıkarın (eject).
4. Terminal'de şu komutu çalıştırın:

   ```bash
   xattr -dr com.apple.quarantine "/Applications/Mikro Cari Aktarim.app"
   ```

5. Uygulamayı açın. (İlk açılışta hâlâ uyarı çıkarsa: uygulamaya **sağ tık → Aç**.)

> Not: Otomatik güncelleme yalnızca Windows'ta çalışır. macOS sürümü elle
> indirilip yukarıdaki adımlarla kurulur.

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

## Denetim kaydı

Uygulama ilk aktarımda `dbo.CARI_AKTARIM_LOG` tablosunu oluşturur. Her başarılı
ve hatalı satır için tarih, uygulamayı çalıştıran bilgisayar/kullanıcı, SQL
oturum kullanıcısı, eski/yeni kod, kart silme tercihi ve sonuç mesajı kaydedilir.
Bu tablo için oluşturma ve ekleme yetkisi zorunludur; denetim kaydı
yazılamıyorsa aktarım başlatılmaz.
