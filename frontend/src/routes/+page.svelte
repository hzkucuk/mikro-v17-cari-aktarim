<script lang="ts">
  import { onMount } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { check } from '@tauri-apps/plugin-updater';
  import { getVersion } from '@tauri-apps/api/app';
  import { invoke, listen } from '$lib/tauri';

  type Row = { eski: string; yeni: string; sil: boolean; status?: string; message?: string };
  type Trigger = { name: string; table: string };
  let cfg = $state({ server: '10.0.0.10', database: 'MikroDesktop_BEDIR_2017_TEST', auth: 'windows', username: '', password: '', trustCert: true });
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
    <label>Cari tipi <select bind:value={cariTipi}><option value={0}>0 — Cari Hesap (müşteri/tedarikçi)</option><option value={1}>1 — Satıcı / Temsilci</option><option value={2}>2 — Banka Hesabı</option><option value={3}>3 — Hizmet</option><option value={4}>4 — Kasa</option><option value={5}>5 — Masraf Merkezi / Gider</option><option value={7}>7 — Personel (bordro)</option><option value={8}>8 — Demirbaş</option><option value={9}>9 — EXIM (ithalat/ihracat)</option></select></label>
  </div><div class="triggers"><b>Yönetilecek trigger’lar</b>{#each triggers as trigger}<div><input placeholder="dbo.trigger" bind:value={trigger.name} /><input placeholder="dbo.TABLO" bind:value={trigger.table} /><button onclick={() => triggers = triggers.filter((x) => x !== trigger)}>×</button></div>{/each}<button onclick={() => triggers = [...triggers, { name: '', table: '' }]}>+ Trigger ekle</button></div><details><summary>Gelişmiş ayarlar</summary><div class="advanced"><label>Aktif User ID <input type="number" min="0" bind:value={userId} /></label><label><input type="checkbox" bind:checked={sonDegGuncelle} /> Son değişiklik bilgilerini güncelle</label></div></details>
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
     Mikro V17 grid görünümü: mor gradient zemin, koyu lacivert panel
     başlıkları, siyah çerçeveler, kod alanlarında Consolas monospace.
     --------------------------------------------------------------- */
  :global(*) { box-sizing: border-box }
  :global(html), :global(body) { height:100%; margin:0 }
  :global(body) {
    background:#e4e8f0;
    color:#1a1a1a;
    font:13px "Segoe UI",Tahoma,sans-serif;
  }

  main { height:100vh; min-height:0; display:flex; flex-direction:column; overflow:hidden }

  /* Başlık şeridi — pencere kenarına dayalı (full-bleed) */
  header {
    display:flex; justify-content:space-between; align-items:center;
    background:#fff; border-bottom:1px solid #000;
    padding:10px clamp(14px,2.4vw,24px);
  }
  h1,h2,p { margin:0 }
  h1 { font-size:17px; font-weight:600; color:#1f2a44 }
  small {
    font:12px Consolas,"Courier New",monospace; color:#666;
    background:#eef0f4; border:1px solid #c8ccd4; border-radius:3px;
    padding:1px 6px; margin-left:6px; vertical-align:middle;
  }
  p { color:#6b7280; margin-top:2px; font-size:11px }
  .version { font-family:Consolas,ui-monospace,monospace; color:#475569 }

  /* Kırmızı yedek uyarı bandı */
  .warning {
    background:#c62828; color:#fff; font-weight:600; letter-spacing:.2px;
    text-align:center; padding:7px clamp(14px,2.4vw,24px);
  }

  /* İçerik alanı — pencereyi tümüyle doldurur, kenarda boşluk yok */
  .workspace { flex:1; min-height:0; display:grid; overflow:auto; background:#c8d0de }

  /* Paneller: beyaz gövde, üst/alt siyah çerçeve (yanlar pencere kenarında) */
  section {
    min-width:0; min-height:0; overflow:auto; position:relative;
    background:#fff; border-top:1px solid #000;
    padding:0 clamp(14px,2.4vw,24px) 14px;
  }

  /* Koyu lacivert panel başlıkları — Mikro grid header'ı */
  h2 {
    font-size:12px; font-weight:600; background:#3c4a5e; color:#fff;
    margin:0 clamp(-24px,-2.4vw,-14px) 14px; padding:6px clamp(14px,2.4vw,24px);
    border-bottom:1px solid #000;
  }

  /* Form alanları */
  .form { display:grid; grid-template-columns:repeat(2,minmax(0,1fr)); gap:12px }
  .form label, .triggers { display:grid; gap:5px; font-weight:600; font-size:12px; color:#333 }
  .picker, .triggers>div { display:flex; gap:6px }

  input, select {
    width:100%; min-height:32px; padding:5px 7px;
    border:1px solid #7a869a; border-radius:0; background:#fff;
    font-family:Consolas,"Courier New",monospace; font-size:12.5px;
  }
  select { font-family:"Segoe UI",Tahoma,sans-serif }
  input:focus, select:focus { outline:2px solid #2563eb; outline-offset:-1px }

  button {
    min-height:32px; cursor:pointer; font-weight:600; font-size:12px;
    padding:6px 14px; border:1px solid rgba(0,0,0,.35); border-radius:2px;
    background:#e4e7ec; color:#1a1a1a; font-family:"Segoe UI",Tahoma,sans-serif;
  }
  button:hover:not(:disabled) { filter:brightness(1.08) }
  button:active:not(:disabled) { transform:translateY(1px) }
  button:disabled { opacity:.45; cursor:not-allowed }

  .primary   { background:#1d6fd0; color:#fff }
  .danger    { background:#c62828; color:#fff }
  .success   { background:#17803d; color:#fff }
  .secondary { background:#4b5563; color:#fff }
  .outline   { background:#b45309; color:#fff; border-color:rgba(0,0,0,.35) }

  .triggers { margin-top:16px }
  .triggers>div input { flex:1 }
  .triggers>div button, .triggers>button { min-height:28px; padding:3px 10px }

  details { margin-top:14px }
  summary { cursor:pointer; font-weight:600; font-size:12px; color:#333 }
  .advanced { display:flex; gap:18px; align-items:center; padding:10px 0 }
  .advanced label { display:flex; align-items:center; gap:7px; font-weight:600; font-size:12px }
  .advanced input[type=number] { width:100px }
  .advanced input[type=checkbox] { width:auto; min-height:auto }

  /* Aksiyon çubukları */
  footer, .section-tools { display:flex; gap:8px; align-items:center; flex-wrap:wrap; margin-top:14px }
  footer {
    padding:10px clamp(14px,2.4vw,24px);
    margin:14px clamp(-24px,-2.4vw,-14px) -14px;
    border-top:1px solid #d0d4dc; background:#f7f8fa;
    /* Panel içeriği taşsa bile aksiyon butonları görünür kalsın. */
    position:sticky; bottom:-14px; z-index:2;
    box-shadow:0 -6px 10px -6px rgba(0,0,0,.18);
  }
  footer span { margin-left:auto; color:#6b7280; font-size:12px }
  .section-tools { margin:0 0 10px; justify-content:flex-end }
  .section-tools button { min-height:26px; padding:2px 9px; font-size:11px; font-weight:500 }

  /* Mikro grid */
  .table { overflow:auto; border:1px solid #000 }
  table { width:100%; min-width:720px; border-collapse:collapse; font-size:12.5px }
  thead th {
    background:#d6dae2; color:#1f2a44; font-weight:600; font-size:11.5px;
    text-align:left; padding:5px 7px; border:1px solid #000; white-space:nowrap;
  }
  tbody td { border:1px solid #9aa2b1; padding:0; background:#fff }
  tbody tr:nth-child(even) td { background:#f4f6f9 }
  tbody td:first-child { text-align:center; font-family:Consolas,monospace; font-size:11.5px; color:#6b7280; padding:5px 0 }
  tbody td input:not([type=checkbox]) {
    border:none; background:transparent; min-height:auto; padding:5px 7px;
  }
  tbody td input:focus { outline:2px solid #2563eb; outline-offset:-2px; background:#fffbe6 }
  tbody td:nth-child(4), tbody td:nth-child(6) { text-align:center }
  tbody td button { min-height:auto; border:none; background:transparent; color:#c62828; font-size:14px; padding:4px 8px }
  tbody td button:hover:not(:disabled) { background:#fde8e8; filter:none }

  .hint { margin-bottom:10px; font-size:11.5px; color:#6b7280 }
  .hint code { font-family:Consolas,monospace; background:#eef0f4; border:1px solid #c8ccd4; padding:1px 4px }
  .hidden { display:none }

  /* Sürüklenebilir ayraç */
  .splitter {
    height:8px; background:#c3b6d8; border-top:1px solid #000; border-bottom:1px solid #000;
    position:relative; cursor:row-resize; touch-action:none;
  }
  .splitter:after { content:'⋮'; position:absolute; left:50%; top:-7px; color:#3c4a5e; font-size:18px; transform:rotate(90deg) }

  /* İlerleme çubuğu */
  .progress { height:14px; background:#e4e7ec; border:1px solid #9aa2b1; overflow:hidden; margin-top:12px }
  .progress span { display:block; height:100%; background:linear-gradient(90deg,#17803d,#22a355); transition:width .2s }
  .progress-text { margin-top:5px; font-family:Consolas,monospace; font-size:11.5px; color:#4b5563 }

  /* Log paneli — koyu konsol */
  .log { padding-bottom:0 }
  .log pre {
    margin:0 clamp(-24px,-2.4vw,-14px); padding:10px clamp(14px,2.4vw,24px);
    min-height:180px; background:#14181f; color:#d7dce4;
    font-family:Consolas,"Courier New",monospace; font-size:11.5px; line-height:1.55;
    white-space:pre-wrap; word-break:break-word;
  }

  /* Orta genişlik (tablet / dar pencere): bağlantı formu iki sütun */
  @media(min-width:701px) and (max-width:1100px){
    .connection .form { grid-template-columns:1fr 1fr; gap:8px 12px }
    .connection .backup-field { grid-column:span 2 }
  }

  /* Geniş ekran: bağlantı formu üç sütun, kompakt satır yükseklikleri */
  @media(min-width:1101px){
    .connection .form { grid-template-columns:1.15fr 1.15fr 1.1fr; gap:8px 12px }
    .connection .backup-field { grid-column:span 2 }
    .connection input, .connection select { min-height:30px; padding:4px 7px }
    .connection .triggers { margin-top:10px; gap:4px }
    .connection details { margin-top:8px }
    .connection .advanced { padding:5px 0 }
  }

  /* SQL önizleme modalı */
  .modal-backdrop {
    position:fixed; inset:0; z-index:100; padding:24px;
    background:rgba(0,0,0,.55); display:flex; align-items:center; justify-content:center;
  }
  .modal {
    background:#fff; border:1px solid #000; width:min(860px,100%);
    max-height:86vh; display:flex; flex-direction:column;
  }
  .modal-head {
    background:#3c4a5e; color:#fff; font-weight:600; font-size:12.5px;
    padding:8px 14px; border-bottom:1px solid #000;
  }
  .modal-note {
    background:#fff3cd; color:#7a5b00; font-size:11.5px; line-height:1.5;
    padding:8px 14px; border-bottom:1px solid #e6d9a8;
  }
  .modal-sql {
    margin:0; padding:12px 14px; overflow:auto; flex:1;
    background:#14181f; color:#d7dce4;
    font-family:Consolas,"Courier New",monospace; font-size:12px; line-height:1.5;
    white-space:pre; tab-size:4;
  }
  .modal-actions {
    display:flex; gap:8px; align-items:center; padding:10px 14px;
    border-top:1px solid #d0d4dc; background:#f7f8fa;
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
