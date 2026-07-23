# Mikro Cari Aktarım — Zorunlu çalışma kontrol listesi

Bu dosya, bu repoda yapılan her değişiklikte uyulacak kalıcı çalışma
talimatıdır. Bir özelliği “tamamlandı” veya “yayına hazır” diye bildirmeden
önce aşağıdaki maddeler uygulanır.

## Arayüz ve kullanılabilirlik

- Ana hedef ekran **1920 × 1080**'dir. Bu çözünürlükte her panelin başlığı,
  içeriği ve tüm işlem düğmeleri görünür olmalıdır; panel, splitter veya
  scrollbar hiçbir düğmeyi kesemez ya da üstüne gelemez.
- İkincil denetimler: 1366 × 768 ve 900 px altı genişlik. Dar ekranda düzen
  tek sütuna geçmeli; yatay taşma yalnızca veri tablosunda kabul edilebilir.
- Her kullanıcı eyleminin görünür sonucu olmalıdır: yükleniyor durumu,
  başarılı/başarısız geri bildirimi veya kullanıcıya gösterilen hata.
- Aynı işlevi yapan düğme birden fazla yerde gösterilmez. Bir düğme eklendiği
  zaman tıklamasının bağlı olduğu komut/işlev ve pasif koşulları doğrulanır.
- Splitter'lar sürüklenebilir olmalı; içerik yüksekliğinin altına inmeyi
  engelleyen minimum değerleri olmalıdır.

## Tauri ve SvelteKit

- Yeni bir ihtiyaçta önce resmi Tauri eklentileri ve resmi dokümantasyon
  kontrol edilir; uygun eklenti yoksa özel kod yazılır.
- Tek arayüz kaynağı `frontend/` (SvelteKit) altındadır. Eski `dist/`
  prototipi kaldırıldı; tekrar oluşturulmamalıdır.
- Her UI değişikliğinden sonra çalıştır:
  `npm --prefix frontend run check` ve `npm --prefix frontend run build`.
- Rust/Tauri değişikliği varsa ayrıca çalıştır:
  `cargo check --manifest-path src-tauri/Cargo.toml`.

## Aktarım güvenliği

- Aktarım, başarılı bağlantı testi ve `COPY_ONLY` SQL yedeği olmadan
  başlatılamaz.
- Trigger'lar aktarımdan önce kapatılır; başarı, hata veya iptal sonrasında
  uygulamanın kapattığı trigger'lar mutlaka geri açılır.
- Trigger geri açma hatası kullanıcıya görünür ve kritik olarak bildirilir.
- SQL denetim kaydı, bilgisayar/kullanıcı/zaman/eski-yeni kod/sonuç bilgileri
  ile yazılmaya devam etmelidir.

## Güncelleme ve yayın

- Uygulama sürümü `src-tauri/Cargo.toml` ve `src-tauri/tauri.conf.json` içinde
  aynı olmalıdır. Her kullanıcıya sunulacak değişiklikte sürüm artırılır ve
  başlıktaki görünür sürüm numarası paket sürümüyle doğrulanır.
- Kullanıcıya yönelik bir committen sonra değişiklik `main` dalına gönderilir;
  Windows build'i başarılı olduğunda aynı sürüm GitHub Release olarak
  yayımlanır. Kullanıcı açıkça “yalnız derle” veya “yayınlama” demedikçe bu
  adım atlanmaz.
- Uygulama açılışında sessiz güncelleme denetimi yapar; yeni sürüm varsa
  kullanıcıya indirme/kurma seçeneği sunar. Elle denetim, “güncel” sonucunu
  da görünür biçimde gösterir.
- Windows paketleri yalnız GitHub Actions `windows-latest` üzerinde derlenir.
  Workflow Node **22** ile `npm ci --prefix frontend` çalıştırmalıdır.
- Release öncesi Actions build'i başarılı olmalı; NSIS `.exe`, `.sig`, MSI ve
  doğru asset URL’si içeren imzalı `latest.json` yayınlanmalıdır.
- `latest.json` içindeki URL’nin yayımlanan asset adıyla eşleştiği ve indirilen
  paketin imzalanan paketle aynı olduğu doğrulanmadan release tamamlanmış
  sayılmaz.
