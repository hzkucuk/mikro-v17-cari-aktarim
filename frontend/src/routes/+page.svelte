<script lang="ts">
  import { onMount } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { check } from '@tauri-apps/plugin-updater';
  import { getVersion } from '@tauri-apps/api/app';
  import { invoke, listen } from '$lib/tauri';

  type Row = { eski: string; yeni: string; sil: boolean; status?: string; message?: string };
  type Trigger = { name: string; table: string };
  // Windows Integrated Auth yalnızca Windows'ta çalışır; macOS/Linux'ta
  // varsayılanı SQL Server Auth yaparak gereksiz bağlantı hatasını önlüyoruz.
  const defaultAuth = (typeof navigator !== 'undefined' && !/Win/i.test(navigator.userAgent)) ? 'sql' : 'windows';
  let cfg = $state({ server: '10.0.0.10', database: 'MikroDesktop_BEDIR_2017_TEST', auth: defaultAuth, username: '', password: '', trustCert: true });
  let backupDirectory = $state('C:\\MikroYedek');
  let triggers = $state<Trigger[]>([{ name: 'dbo.tr_Siparis_ForinsertUpdate', table: 'dbo.SIPARISLER' }]);
  let rows = $state<Row[]>([{ eski: '', yeni: '', sil: true }]);
  let logs = $state<string[]>(['Uygulama hazır. Önce bağlantıyı test edin.']);
  let connectionOk = $state(false), backupOk = $state(false), running = $state(false), updateBusy = $state(false), info = $state('Bağlantı test edilmedi');
  let progress = $state({ done: 0, total: 0 });
  let csvInput: HTMLInputElement;
  let cariTipi = $state(0), userId = $state(1), sonDegGuncelle = $state(false);
  let previewSql = $state<string | null>(null), previewBusy = $state(false);
  let appVersion = $state('v0.1.8');
  // İlk değerler panel içeriklerinin tamamını (özellikle çoklu trigger'ı)
  // splitter altında kesmeden gösterecek şekilde seçildi.
  let panelHeights = $state([390, 270]);
  const log = (m: string) => { logs = [...logs, `${new Date().toLocaleTimeString('tr-TR')}  ${m}`]; };
  const reset = () => { connectionOk = false; backupOk = false; };
  const cleanTriggers = () => triggers.filter((t) => t.name.trim() || t.table.trim());
  const cleanRows = () => rows.filter((r) => r.eski.trim() || r.yeni.trim());

  onMount(() => {
    let unlistenRow: (() => void) | undefined;
    let unlistenLog: (() => void) | undefined;
    let unlistenProgress: (() => void) | undefined;
    const fitPanels = () => {
      // 1920 × 1080 ekran için: başlık/uyarıdan sonra üç panel de görünür
      // kalır. Daha kısa ekranlarda kendi içlerinde kaydırılabilirler.
      const workspaceHeight = Math.max(720, window.innerHeight - 104);
      panelHeights = [Math.min(350, Math.max(290, Math.round(workspaceHeight * 0.33))), Math.min(285, Math.max(230, Math.round(workspaceHeight * 0.27)))];
    };
    fitPanels(); window.addEventListener('resize', fitPanels);
    void getVersion().then((version) => appVersion = `v${version}`).catch(() => undefined);
    void (async () => {
      unlistenRow = await listen<Row & { index: number }>('row-status', ({ payload }) => {
        rows[payload.index] = { ...rows[payload.index], status: payload.status, message: payload.message };
      });
      unlistenLog = await listen<string>('log', ({ payload }) => log(payload));
      unlistenProgress = await listen<{ done: number; total: number }>('progress', ({ payload }) => progress = payload);
    })();
    // Açılışta otomatik güncelleme denetimi yapmıyoruz: güncelleme sunucusu
    // yapılandırılmadığı için her açılışta log'a hata düşerdi. Kullanıcı
    // "Güncelleme Denetle" ile elle tetikler.
    return () => { window.removeEventListener('resize', fitPanels); unlistenRow?.(); unlistenLog?.(); unlistenProgress?.(); };
  });

  async function testConnection() {
    try { info = 'Bağlanılıyor…'; const r = await invoke<string>('test_connection', { cfg }); connectionOk = true; backupOk = false; info = 'Bağlantı hazır — yedek alınmalı'; log(r); }
    catch (e) { connectionOk = false; info = `Bağlantı hatası: ${e}`; log(info); }
  }
  async function chooseFolder() { const p = await open({ directory: true, multiple: false, title: 'SQL Server yedek klasörü' }); if (p) { backupDirectory = p; reset(); } }
  async function backup() {
    try { info = 'Yedek alınıyor…'; const r = await invoke<{ message: string }>('backup_database', { cfg, backupDirectory }); backupOk = true; info = 'Yedek hazır — aktarım yapılabilir'; log(r.message); }
    catch (e) { backupOk = false; info = `Yedek hatası: ${e}`; log(info); }
  }
  // Aktarım için ortak doğrulama; geçerliyse temizlenmiş satır/trigger döner.
  function validateForTransfer() {
    const activeRows = cleanRows(), activeTriggers = cleanTriggers();
    if (!activeRows.length || activeRows.some((r) => !r.eski || !r.yeni)) { log('Eski ve yeni kodları doldurun.'); return null; }
    if (activeTriggers.some((t) => !t.name || !t.table)) { log('Her trigger için ad ve tablo girin.'); return null; }
    return { activeRows, activeTriggers };
  }
  // "Aktarımı Başlat" → önce çalıştırılacak SQL'i backend'den al ve modalda göster.
  async function openPreview() {
    const v = validateForTransfer(); if (!v) return;
    previewBusy = true; info = 'SQL önizlemesi hazırlanıyor…';
    try {
      previewSql = await invoke<string>('preview_transfer_sql', { cfg, triggers: v.activeTriggers, rows: v.activeRows, cariTipi, userId, sonDegGuncelle });
      info = 'SQL önizlemesi hazır — onayınız bekleniyor';
    } catch (e) { previewSql = null; info = `Önizleme hatası: ${e}`; log(info); alert(info); }
    finally { previewBusy = false; }
  }
  // Modaldaki "Onayla ve Aktar" → gerçek aktarımı çalıştır.
  async function confirmTransfer() {
    const v = validateForTransfer(); if (!v) { previewSql = null; return; }
    previewSql = null;
    running = true; progress = { done: 0, total: v.activeRows.length }; rows = rows.map((r) => ({ ...r, status: '', message: '' }));
    try { const s = await invoke<{ total: number; ok: number; failed: number; triggerMessage: string; triggerRestored: boolean; errors: string[] }>('run_transfer', { cfg, triggers: v.activeTriggers, rows: v.activeRows, cariTipi, userId, sonDegGuncelle }); const message = `Aktarım bitti: ${s.ok} başarılı, ${s.failed} hatalı.\n${s.triggerMessage}${s.errors.length ? `\n\nHatalar:\n${s.errors.join('\n')}` : ''}`; log(message); alert(message); }
    catch (e) { log(`Aktarım hatası: ${e}`); } finally { running = false; }
  }
  async function copyPreview() { try { await navigator.clipboard.writeText(previewSql ?? ''); log('SQL panoya kopyalandı.'); } catch { log('Panoya kopyalanamadı.'); } }
  async function triggerStatus() { try { log(await invoke<string>('trigger_status', { cfg, triggers: cleanTriggers() })); } catch (e) { log(`Trigger durumu alınamadı: ${e}`); } }
  async function enableTriggers() { if (!confirm('Tanımlı trigger’lar etkinleştirilsin mi? Bu yalnızca acil kurtarma içindir.')) return; try { log(await invoke<string>('enable_trigger', { cfg, triggers: cleanTriggers() })); } catch (e) { log(`Trigger etkinleştirilemedi: ${e}`); } }
  async function cancel() { await invoke<void>('cancel_transfer'); log('İptal isteği gönderildi; işlemdeki satır tamamlandıktan sonra durur.'); }
  function parseCsv(text: string) {
    const first = text.replace(/^\uFEFF/, '').split(/\r?\n/, 1)[0] ?? '';
    const delimiter = [';', ',', '\t'].reduce((best, value) => first.split(value).length > first.split(best).length ? value : best, ';');
    const output: string[][] = []; let row: string[] = [], value = '', quoted = false;
    for (let i = 0; i < text.length; i += 1) { const char = text[i]; if (char === '"') { if (quoted && text[i + 1] === '"') { value += '"'; i += 1; } else quoted = !quoted; } else if (!quoted && char === delimiter) { row.push(value.trim()); value = ''; } else if (!quoted && (char === '\n' || char === '\r')) { if (char === '\r' && text[i + 1] === '\n') i += 1; row.push(value.trim()); if (row.some(Boolean)) output.push(row); row = []; value = ''; } else value += char; }
    row.push(value.trim()); if (row.some(Boolean)) output.push(row); return output;
  }
  function importCsv(event: Event) {
    const input = event.currentTarget as HTMLInputElement;
    const file = input.files?.[0]; if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      const parsed = parseCsv(String(reader.result).replace(/^\uFEFF/, ''));
      const data = /eski|kaynak/i.test(parsed[0]?.[0] ?? '') || /yeni|hedef/i.test(parsed[0]?.[1] ?? '') ? parsed.slice(1) : parsed;
      const incomplete = data.find((cells) => (cells[0] || cells[1]) && (!cells[0] || !cells[1]));
      if (incomplete) return log('CSV’de eski veya yeni cari kodu boş olan bir satır var.');
      const imported = data.filter((cells) => cells[0] || cells[1]).map((cells) => ({ eski: cells[0], yeni: cells[1], sil: !/^(0|hayır|hayir|false|no|kalsın|kalsin)$/i.test(cells[2] ?? '') }));
      if (imported.length) { rows = rows.length === 1 && !rows[0].eski && !rows[0].yeni ? imported : [...rows, ...imported]; log(`${imported.length} CSV satırı içe aktarıldı: ${file.name}`); } else log('CSV’de aktarılacak satır bulunamadı.');
    }; reader.readAsText(file, 'utf-8'); input.value = '';
  }
  async function update(interactive = true) {
    if (updateBusy) return;
    updateBusy = true; if (interactive) info = 'Güncelleme denetleniyor…'; log('Güncelleme denetleniyor…');
    try {
      const u = await check();
      if (!u) { info = 'Uygulama güncel'; log('Uygulama güncel.'); if (interactive) alert('Uygulama güncel.'); return; }
      const install = confirm(`v${u.version} sürümü bulundu.${u.body ? `\n\nSürüm notları:\n${u.body}` : ''}\n\nİndirip kurmak ister misiniz?`);
      if (!install) { info = `v${u.version} kurulmaya hazır`; return; }
      info = `v${u.version} indiriliyor…`; log(`v${u.version} indiriliyor…`);
      await u.downloadAndInstall();
    } catch (e) {
      const message = `Güncelleme denetimi hatası: ${e}`; log(message); if (interactive) alert(message);
    } finally { updateBusy = false; }
  }
  function resizePanel(index: number, event: PointerEvent) {
    const startY = event.clientY, start = panelHeights[index], min = index === 0 ? 280 : 220;
    const move = (e: PointerEvent) => { panelHeights[index] = Math.max(min, start + e.clientY - startY); };
    const end = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', end); };
    window.addEventListener('pointermove', move); window.addEventListener('pointerup', end);
  }
</script>

<svelte:head><title>Mikro Cari Kartı Aktarma</title></svelte:head>
<main>
  <header><div><h1>Cari Kartı Aktarma <small>098492</small></h1><p>Mikro V17 · Trigger Yönetimli · <span class="version">{appVersion}</span></p></div></header>
  <div class="warning">⚠ ÖNCE YEDEK ALIN — Aktarım geri alınamaz. Aktarım sırasında Mikro’yu kapatın.</div>
  <div class="workspace" style={`grid-template-rows: ${panelHeights[0]}px 8px ${panelHeights[1]}px 8px minmax(180px, 1fr)`}>
  <section class="connection"><h2>Bağlantı ayarları</h2><div class="form">
    <label>Sunucu <input bind:value={cfg.server} oninput={reset} /></label><label>Veritabanı <input bind:value={cfg.database} oninput={reset} /></label>
    <label class="auth-field">Kimlik doğrulama <span><select bind:value={cfg.auth} onchange={reset}><option value="windows">Windows Integrated</option><option value="sql">SQL Server Auth</option></select>{#if cfg.auth === 'sql'} <input placeholder="Kullanıcı" bind:value={cfg.username} /><input type="password" placeholder="Şifre" bind:value={cfg.password} />{/if}</span></label>
    <label class="backup-field">Yedek klasörü <span class="picker"><input bind:value={backupDirectory} oninput={reset} /><button onclick={chooseFolder}>Seç…</button></span></label>
  </div><div class="triggers"><b>Yönetilecek trigger’lar</b>{#each triggers as trigger}<div><input placeholder="dbo.trigger" bind:value={trigger.name} /><input placeholder="dbo.TABLO" bind:value={trigger.table} /><button onclick={() => triggers = triggers.filter((x) => x !== trigger)}>×</button></div>{/each}<button onclick={() => triggers = [...triggers, { name: '', table: '' }]}>+ Trigger ekle</button></div><details><summary>Gelişmiş ayarlar</summary><div class="advanced"><label class="cari-tipi">Cari tipi <select bind:value={cariTipi}><option value={0}>0 — Cari Hesap (müşteri/tedarikçi)</option><option value={1}>1 — Satıcı / Temsilci</option><option value={2}>2 — Banka Hesabı</option><option value={3}>3 — Hizmet</option><option value={4}>4 — Kasa</option><option value={5}>5 — Masraf Merkezi / Gider</option><option value={7}>7 — Personel (bordro)</option><option value={8}>8 — Demirbaş</option><option value={9}>9 — EXIM (ithalat/ihracat)</option></select></label><label>Aktif User ID <input type="number" min="0" bind:value={userId} /></label><label><input type="checkbox" bind:checked={sonDegGuncelle} /> Son değişiklik bilgilerini güncelle</label></div></details>
  <footer><button class="primary" onclick={testConnection} disabled={running}>Bağlantıyı Test Et</button><button class="danger" onclick={backup} disabled={!connectionOk || running}>Önce Yedek Al</button><button class="secondary" onclick={() => update(true)} disabled={running || updateBusy}>{updateBusy ? 'Güncelleme Denetleniyor…' : 'Güncelleme Denetle'}</button><button onclick={triggerStatus} disabled={running}>Trigger Durumu</button><button class="outline" onclick={enableTriggers} disabled={running}>Trigger’ı Geri Aç</button><span>{info}</span></footer></section>
  <div class="splitter" role="separator" aria-label="Bağlantı ve aktarım alanlarının yüksekliğini ayarla" onpointerdown={(event) => resizePanel(0, event)}></div>
  <section class="transfer"><h2>Aktarılacak cari kartları</h2><p class="hint">CSV sütunları: <code>Eski Cari Kodu; Yeni Cari Kodu; Eski Kart Silinsin</code>. İlk iki sütun zorunludur.</p><input class="hidden" bind:this={csvInput} type="file" accept=".csv,text/csv" onchange={importCsv} /><div class="section-tools"><button onclick={() => csvInput.click()}>CSV İçeri Aktar</button><button onclick={() => rows = [...rows, { eski: '', yeni: '', sil: true }]}>+ Satır</button><button onclick={() => rows = [{ eski: '', yeni: '', sil: true }]}>Temizle</button></div><div class="table"><table><thead><tr><th>#</th><th>Eski cari kodu</th><th>Yeni cari kodu</th><th>Eski kart silinsin</th><th>Durum</th><th></th></tr></thead><tbody>{#each rows as row, index}<tr><td>{index + 1}</td><td><input bind:value={row.eski} /></td><td><input bind:value={row.yeni} /></td><td><input type="checkbox" bind:checked={row.sil} /></td><td title={row.message}>{row.status === 'ok' ? '✓ Başarılı' : row.status === 'error' ? '✗ Hata' : row.status === 'running' ? 'İşleniyor…' : row.message || '—'}</td><td><button aria-label="Satırı sil" onclick={() => rows = rows.length === 1 ? [{ eski: '', yeni: '', sil: true }] : rows.filter((x) => x !== row)}>×</button></td></tr>{/each}</tbody></table></div>{#if running}<div class="progress"><span style={`width:${progress.total ? (progress.done / progress.total) * 100 : 0}%`}></span></div><p class="progress-text">{progress.done}/{progress.total} satır işlendi</p>{/if}<footer><button class="success" onclick={openPreview} disabled={!connectionOk || !backupOk || running || previewBusy}>{previewBusy ? 'SQL Hazırlanıyor…' : 'Aktarımı Başlat'}</button><button class="danger" onclick={cancel} disabled={!running}>İptal</button></footer></section>
  <div class="splitter" role="separator" aria-label="Aktarım ve günlük alanlarının yüksekliğini ayarla" onpointerdown={(event) => resizePanel(1, event)}></div>
  <section class="log"><h2>İşlem günlüğü</h2><div class="section-tools"><button onclick={() => navigator.clipboard.writeText(logs.join('\n'))}>Kopyala</button><button onclick={() => logs = []}>Temizle</button></div><pre>{logs.join('\n')}</pre></section>
  </div>
</main>

{#if previewSql !== null}
  <div class="modal-backdrop" role="dialog" aria-modal="true" aria-label="SQL önizlemesi">
    <div class="modal">
      <div class="modal-head">Çalıştırılacak SQL — Onayınız bekleniyor</div>
      <div class="modal-note">⚠ Aşağıdaki adımlar aynen çalıştırılacaktır (her satır kendi transaction'ında). Yedek aldığınızdan emin olun. Gerçek yürütme parametreli sorgu kullanır; değerler burada yalnızca okunabilirlik için gömülmüştür.</div>
      <pre class="modal-sql">{previewSql}</pre>
      <div class="modal-actions">
        <button onclick={copyPreview}>Kopyala</button>
        <span class="spacer"></span>
        <button class="secondary" onclick={() => previewSql = null}>Vazgeç</button>
        <button class="success" onclick={confirmTransfer}>Onayla ve Aktar</button>
      </div>
    </div>
  </div>
{/if}

<style>
  /* ---------------------------------------------------------------
     Temiz / modern arayüz: sistem fontu, yumuşak açık palet, ince
     kenarlıklar, küçük radius, mavi vurgu (#0a5cff). Monospace yalnızca
     kod/kimlik alanlarında (sunucu, kod, trigger, SQL, günlük).
     --------------------------------------------------------------- */
  :global(*) { box-sizing: border-box }
  :global(html), :global(body) { height:100%; margin:0 }
  :global(body) {
    background:#eef1f4;
    color:#1a1a1a;
    font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;
    font-size:13px;
    -webkit-font-smoothing:antialiased;
  }

  main { height:100vh; min-height:0; display:flex; flex-direction:column; overflow:hidden }

  /* Üst şerit — hafif gradient toolbar */
  header {
    display:flex; justify-content:space-between; align-items:center;
    background:linear-gradient(180deg,#ffffff 0%,#f2f4f7 100%);
    border-bottom:1px solid #d9dce1; box-shadow:0 1px 2px rgba(0,0,0,.04);
    padding:10px clamp(14px,2.4vw,24px);
  }
  h1,h2,p { margin:0 }
  h1 { font-size:15px; font-weight:600; color:#1f2937; letter-spacing:-.01em }
  small {
    font:600 11px ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;
    color:#0a5cff; background:rgba(10,92,255,.08); border:1px solid rgba(10,92,255,.18);
    border-radius:5px; padding:1px 7px; margin-left:8px; vertical-align:middle;
  }
  p { color:#6b7280; margin-top:3px; font-size:11px }
  .version { font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; color:#475569 }

  /* Yedek uyarısı — sakin ama net (yumuşak kırmızı bant) */
  .warning {
    background:#fef2f2; color:#b42318; font-weight:600;
    text-align:center; padding:8px clamp(14px,2.4vw,24px);
    border-bottom:1px solid #fbcfc9;
  }

  /* İçerik alanı — pencereyi tümüyle doldurur */
  .workspace { flex:1; min-height:0; display:grid; overflow:auto; background:#eef1f4 }

  /* Paneller: beyaz gövde, ince üst kenarlık */
  section {
    min-width:0; min-height:0; overflow:auto; position:relative;
    background:#fff; border-top:1px solid #e2e5ea;
    padding:0 clamp(14px,2.4vw,24px) 14px;
  }

  /* Panel başlıkları — açık, minik büyük harf etiket */
  h2 {
    font-size:11px; font-weight:600; text-transform:uppercase; letter-spacing:.04em;
    color:#64748b; background:#f8fafc;
    margin:0 clamp(-24px,-2.4vw,-14px) 14px; padding:9px clamp(14px,2.4vw,24px);
    border-bottom:1px solid #e5e7eb;
  }

  /* Form alanları */
  .form { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:14px; align-items:start }
  .form label, .triggers { display:grid; gap:6px; font-weight:600; font-size:12px; color:#475569 }
  .picker, .triggers>div { display:flex; gap:6px }
  .auth-field span { display:flex; gap:6px; align-items:center; flex-wrap:wrap }
  .auth-field select { flex:1 1 180px }

  input, select {
    width:100%; min-height:34px; padding:6px 10px;
    border:1px solid #cbd0d8; border-radius:6px; background:#fff;
    color:#1a1a1a; font-size:13px;
    font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;
    transition:border-color .12s, box-shadow .12s;
  }
  select { font-family:inherit }
  input::placeholder { color:#9099a5 }
  input:focus, select:focus {
    outline:none; border-color:#0a5cff; box-shadow:0 0 0 3px rgba(10,92,255,.14);
  }

  button {
    min-height:34px; cursor:pointer; font-weight:500; font-size:12.5px;
    padding:6px 13px; border:1px solid #cbd0d8; border-radius:6px;
    background:#fff; color:#374151;
    transition:background .12s, border-color .12s, box-shadow .12s, transform .05s;
  }
  button:hover:not(:disabled) { background:#f3f4f6; border-color:#b8bec7 }
  button:active:not(:disabled) { transform:translateY(.5px) }
  button:disabled { opacity:.45; cursor:not-allowed }
  button:focus-visible { outline:none; box-shadow:0 0 0 3px rgba(10,92,255,.22) }

  .primary   { background:#0a5cff; border-color:#0a5cff; color:#fff; font-weight:600 }
  .primary:hover:not(:disabled)   { background:#0847c9; border-color:#0847c9 }
  .success   { background:#16a34a; border-color:#16a34a; color:#fff; font-weight:600 }
  .success:hover:not(:disabled)   { background:#15803d; border-color:#15803d }
  .danger    { background:#dc2626; border-color:#dc2626; color:#fff; font-weight:600 }
  .danger:hover:not(:disabled)    { background:#b91c1c; border-color:#b91c1c }
  .secondary { background:#fff; border-color:#cbd0d8; color:#475569 }
  .outline   { background:#fff; border-color:#fcd34d; color:#b45309 }
  .outline:hover:not(:disabled)   { background:#fffbeb; border-color:#fbbf24 }

  .triggers { margin-top:16px }
  .triggers b { font-weight:600; font-size:12px; color:#475569 }
  .triggers>div input { flex:1 }
  .triggers>div button { min-height:34px; padding:0 12px; color:#dc2626; border-color:#e6c9c9 }
  .triggers>div button:hover:not(:disabled) { background:#fef2f2; border-color:#f0b4b4 }
  .triggers>button { justify-self:start; min-height:30px; padding:5px 12px }

  details { margin-top:14px }
  summary { cursor:pointer; font-weight:600; font-size:12px; color:#475569; padding:4px 0 }
  .advanced { display:flex; gap:18px; align-items:center; padding:10px 0; flex-wrap:wrap }
  .advanced label { display:flex; align-items:center; gap:7px; font-weight:600; font-size:12px; color:#475569 }
  .advanced input[type=number] { width:100px }
  .advanced input[type=checkbox] { width:auto; min-height:auto }
  .advanced .cari-tipi { flex-direction:column; align-items:flex-start; gap:5px }
  .advanced .cari-tipi select { min-width:260px }

  /* Aksiyon çubukları */
  footer, .section-tools { display:flex; gap:8px; align-items:center; flex-wrap:wrap; margin-top:14px }
  footer {
    padding:12px clamp(14px,2.4vw,24px);
    margin:14px clamp(-24px,-2.4vw,-14px) -14px;
    border-top:1px solid #e5e7eb; background:#fbfcfd;
    /* Panel içeriği taşsa bile aksiyon butonları görünür kalsın. */
    position:sticky; bottom:-14px; z-index:2;
    box-shadow:0 -6px 12px -8px rgba(0,0,0,.12);
  }
  footer span { margin-left:auto; color:#6b7280; font-size:12px }
  .section-tools { margin:0 0 10px; justify-content:flex-end }
  .section-tools button { min-height:28px; padding:3px 11px; font-size:12px }

  /* Veri tablosu — hafif, yalnızca satır ayırıcılar */
  .table { overflow:auto; border:1px solid #e2e5ea; border-radius:8px }
  table { width:100%; min-width:720px; border-collapse:collapse; font-size:13px }
  thead th {
    background:#f8fafc; color:#64748b; font-weight:600; font-size:11px;
    text-transform:uppercase; letter-spacing:.03em; text-align:left;
    padding:9px 10px; border-bottom:1px solid #e5e7eb; white-space:nowrap;
  }
  tbody td { border-bottom:1px solid #eef1f4; padding:0; background:#fff }
  tbody tr:last-child td { border-bottom:none }
  tbody tr:hover td { background:#f8fafc }
  tbody td:first-child { text-align:center; font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; font-size:12px; color:#9099a5; padding:7px 0; width:44px }
  tbody td input:not([type=checkbox]) {
    border:none; border-radius:0; background:transparent; min-height:auto; padding:8px 10px; box-shadow:none;
  }
  tbody td input:focus { outline:none; background:#eff5ff; box-shadow:inset 0 0 0 2px rgba(10,92,255,.25) }
  tbody td:nth-child(4), tbody td:nth-child(6) { text-align:center }
  tbody td button { min-height:auto; border:none; background:transparent; color:#dc2626; font-size:15px; padding:4px 9px; border-radius:5px }
  tbody td button:hover:not(:disabled) { background:#fef2f2 }

  .hint { margin-bottom:10px; font-size:12px; color:#6b7280 }
  .hint code { font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; background:#eef1f5; border:1px solid #dfe3e9; border-radius:4px; padding:1px 5px; font-size:11.5px }
  .hidden { display:none }

  /* Sürüklenebilir ayraç — ince, tutamak çubuklu */
  .splitter {
    height:9px; background:#eef1f4; border-top:1px solid #e2e5ea; border-bottom:1px solid #e2e5ea;
    position:relative; cursor:row-resize; touch-action:none;
  }
  .splitter:hover { background:#e2e6ec }
  .splitter:after {
    content:''; position:absolute; left:50%; top:50%; width:30px; height:3px;
    margin:-1.5px 0 0 -15px; border-radius:999px; background:#c3cad4;
  }

  /* İlerleme çubuğu */
  .progress { height:8px; background:#e5e8ec; border-radius:999px; overflow:hidden; margin-top:12px }
  .progress span { display:block; height:100%; background:#0a5cff; border-radius:999px; transition:width .2s }
  .progress-text { margin-top:6px; font-size:11.5px; color:#6b7280 }

  /* Günlük — koyu konsol */
  .log { padding-bottom:0 }
  .log pre {
    margin:0 clamp(-24px,-2.4vw,-14px); padding:12px clamp(14px,2.4vw,24px);
    min-height:180px; background:#1e293b; color:#cbd5e1;
    font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; font-size:12px; line-height:1.6;
    white-space:pre-wrap; word-break:break-word;
  }

  /* Orta genişlik (tablet / dar pencere): bağlantı formu iki sütun */
  @media(min-width:701px) and (max-width:1100px){
    .connection .form { grid-template-columns:1fr 1fr; gap:10px 14px }
    .connection .backup-field { grid-column:span 2 }
  }

  /* Geniş ekran: bağlantı formu üç sütun, kompakt satır yükseklikleri */
  @media(min-width:1101px){
    .connection .form { grid-template-columns:1.15fr 1.15fr 1.1fr; gap:10px 14px }
    .connection .backup-field { grid-column:span 2 }
    .connection input, .connection select { min-height:32px; padding:5px 9px }
    .connection .triggers { margin-top:12px; gap:5px }
    .connection details { margin-top:10px }
    .connection .advanced { padding:6px 0 }
  }

  /* SQL önizleme modalı */
  .modal-backdrop {
    position:fixed; inset:0; z-index:100; padding:24px;
    background:rgba(15,23,42,.5); display:flex; align-items:center; justify-content:center;
    backdrop-filter:blur(2px);
  }
  .modal {
    background:#fff; border:1px solid #e2e5ea; border-radius:10px; width:min(860px,100%);
    max-height:86vh; display:flex; flex-direction:column; overflow:hidden;
    box-shadow:0 20px 50px -12px rgba(0,0,0,.35);
  }
  .modal-head {
    background:#f8fafc; color:#1f2937; font-weight:600; font-size:13px;
    padding:12px 16px; border-bottom:1px solid #e5e7eb;
  }
  .modal-note {
    background:#fffbeb; color:#92400e; font-size:12px; line-height:1.5;
    padding:10px 16px; border-bottom:1px solid #fde68a;
  }
  .modal-sql {
    margin:0; padding:14px 16px; overflow:auto; flex:1;
    background:#1e293b; color:#cbd5e1;
    font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; font-size:12px; line-height:1.55;
    white-space:pre; tab-size:4;
  }
  .modal-actions {
    display:flex; gap:8px; align-items:center; padding:12px 16px;
    border-top:1px solid #e5e7eb; background:#fbfcfd;
  }
  .modal-actions .spacer { flex:1 }

  /* Dar ekran: paneller alt alta, ayraçlar gizli, tek sütun */
  @media(max-width:700px){
    main { height:auto; min-height:100vh; overflow:visible }
    .workspace { display:block!important; overflow:visible }
    .splitter { display:none }
    .connection, .transfer, .log { overflow:visible; margin-bottom:8px }
    .form { grid-template-columns:1fr }
    header { align-items:flex-start; gap:8px; flex-direction:column }
    .advanced { align-items:flex-start; flex-direction:column }
    footer { position:static; box-shadow:none }
    .picker, .triggers>div { flex-wrap:wrap }
  }
</style>
