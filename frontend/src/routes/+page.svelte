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
  let appVersion = $state('v0.1.7');
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
      panelHeights = [Math.min(430, Math.max(360, Math.round(workspaceHeight * 0.40))), Math.min(300, Math.max(245, Math.round(workspaceHeight * 0.28)))];
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
    const updateTimer = window.setTimeout(() => void update(false), 900);
    return () => { window.clearTimeout(updateTimer); window.removeEventListener('resize', fitPanels); unlistenRow?.(); unlistenLog?.(); unlistenProgress?.(); };
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
  async function transfer() {
    const activeRows = cleanRows(), activeTriggers = cleanTriggers();
    if (!activeRows.length || activeRows.some((r) => !r.eski || !r.yeni)) return log('Eski ve yeni kodları doldurun.');
    if (activeTriggers.some((t) => !t.name || !t.table)) return log('Her trigger için ad ve tablo girin.');
    if (!confirm(`${activeRows.length} satır aktarılacak. Yedek alındı mı?`)) return;
    running = true; progress = { done: 0, total: activeRows.length }; rows = rows.map((r) => ({ ...r, status: '', message: '' }));
    try { const s = await invoke<{ total: number; ok: number; failed: number; triggerMessage: string; triggerRestored: boolean; errors: string[] }>('run_transfer', { cfg, triggers: activeTriggers, rows: activeRows, cariTipi, userId, sonDegGuncelle }); const message = `Aktarım bitti: ${s.ok} başarılı, ${s.failed} hatalı.\n${s.triggerMessage}${s.errors.length ? `\n\nHatalar:\n${s.errors.join('\n')}` : ''}`; log(message); alert(message); }
    catch (e) { log(`Aktarım hatası: ${e}`); } finally { running = false; }
  }
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
    const startY = event.clientY, start = panelHeights[index], min = index === 0 ? 470 : 270;
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
    <label>Yedek klasörü <span class="picker"><input bind:value={backupDirectory} oninput={reset} /><button onclick={chooseFolder}>Seç…</button></span></label>
    <label>Kimlik doğrulama <span><select bind:value={cfg.auth} onchange={reset}><option value="windows">Windows Integrated</option><option value="sql">SQL Server Auth</option></select>{#if cfg.auth === 'sql'} <input placeholder="Kullanıcı" bind:value={cfg.username} /><input type="password" placeholder="Şifre" bind:value={cfg.password} />{/if}</span></label>
    <label>Cari tipi <select bind:value={cariTipi}><option value={0}>0 — Cari Hesap (müşteri/tedarikçi)</option><option value={1}>1 — Satıcı / Temsilci</option><option value={2}>2 — Banka Hesabı</option><option value={3}>3 — Hizmet</option><option value={4}>4 — Kasa</option><option value={5}>5 — Masraf Merkezi / Gider</option><option value={7}>7 — Personel (bordro)</option><option value={8}>8 — Demirbaş</option><option value={9}>9 — EXIM (ithalat/ihracat)</option></select></label>
  </div><div class="triggers"><b>Yönetilecek trigger’lar</b>{#each triggers as trigger}<div><input placeholder="dbo.trigger" bind:value={trigger.name} /><input placeholder="dbo.TABLO" bind:value={trigger.table} /><button onclick={() => triggers = triggers.filter((x) => x !== trigger)}>×</button></div>{/each}<button onclick={() => triggers = [...triggers, { name: '', table: '' }]}>+ Trigger ekle</button></div><details><summary>Gelişmiş ayarlar</summary><div class="advanced"><label>Aktif User ID <input type="number" min="0" bind:value={userId} /></label><label><input type="checkbox" bind:checked={sonDegGuncelle} /> Son değişiklik bilgilerini güncelle</label></div></details>
  <footer><button class="primary" onclick={testConnection} disabled={running}>Bağlantıyı Test Et</button><button class="danger" onclick={backup} disabled={!connectionOk || running}>Önce Yedek Al</button><button class="secondary" onclick={() => update(true)} disabled={running || updateBusy}>{updateBusy ? 'Güncelleme Denetleniyor…' : 'Güncelleme Denetle'}</button><button onclick={triggerStatus} disabled={running}>Trigger Durumu</button><button class="outline" onclick={enableTriggers} disabled={running}>Trigger’ı Geri Aç</button><span>{info}</span></footer></section>
  <div class="splitter" role="separator" aria-label="Bağlantı ve aktarım alanlarının yüksekliğini ayarla" onpointerdown={(event) => resizePanel(0, event)}></div>
  <section class="transfer"><h2>Aktarılacak cari kartları</h2><p class="hint">CSV sütunları: <code>Eski Cari Kodu; Yeni Cari Kodu; Eski Kart Silinsin</code>. İlk iki sütun zorunludur.</p><input class="hidden" bind:this={csvInput} type="file" accept=".csv,text/csv" onchange={importCsv} /><div class="section-tools"><button onclick={() => csvInput.click()}>CSV İçeri Aktar</button><button onclick={() => rows = [...rows, { eski: '', yeni: '', sil: true }]}>+ Satır</button><button onclick={() => rows = [{ eski: '', yeni: '', sil: true }]}>Temizle</button></div><div class="table"><table><thead><tr><th>#</th><th>Eski cari kodu</th><th>Yeni cari kodu</th><th>Eski kart silinsin</th><th>Durum</th><th></th></tr></thead><tbody>{#each rows as row, index}<tr><td>{index + 1}</td><td><input bind:value={row.eski} /></td><td><input bind:value={row.yeni} /></td><td><input type="checkbox" bind:checked={row.sil} /></td><td title={row.message}>{row.status === 'ok' ? '✓ Başarılı' : row.status === 'error' ? '✗ Hata' : row.status === 'running' ? 'İşleniyor…' : row.message || '—'}</td><td><button aria-label="Satırı sil" onclick={() => rows = rows.length === 1 ? [{ eski: '', yeni: '', sil: true }] : rows.filter((x) => x !== row)}>×</button></td></tr>{/each}</tbody></table></div>{#if running}<div class="progress"><span style={`width:${progress.total ? (progress.done / progress.total) * 100 : 0}%`}></span></div><p class="progress-text">{progress.done}/{progress.total} satır işlendi</p>{/if}<footer><button class="success" onclick={transfer} disabled={!connectionOk || !backupOk || running}>Aktarımı Başlat</button><button class="danger" onclick={cancel} disabled={!running}>İptal</button></footer></section>
  <div class="splitter" role="separator" aria-label="Aktarım ve günlük alanlarının yüksekliğini ayarla" onpointerdown={(event) => resizePanel(1, event)}></div>
  <section class="log"><h2>İşlem günlüğü</h2><div class="section-tools"><button onclick={() => navigator.clipboard.writeText(logs.join('\n'))}>Kopyala</button><button onclick={() => logs = []}>Temizle</button></div><pre>{logs.join('\n')}</pre></section>
  </div>
</main>

<style>
  :global(*) { box-sizing: border-box } :global(html),:global(body) { height:100%;margin:0 } :global(body) { background:#f5f7fb;color:#172033;font:14px Inter,Segoe UI,sans-serif } main { height:100vh;min-height:720px;display:flex;flex-direction:column;overflow:hidden } header,section,.warning { padding:16px clamp(16px,3vw,44px) } header { display:flex;justify-content:space-between;align-items:center;background:white;border-bottom:1px solid #dce3ef } h1,h2,p { margin:0 } h1{font-size:19px} small{font:12px monospace;background:#e8edf5;padding:2px 6px;border-radius:4px} p{color:#64748b;margin-top:3px;font-size:12px}.version{font-family:ui-monospace,SFMono-Regular,Consolas,monospace;color:#475569}.warning{background:#b42318;color:white;text-align:center;font-weight:700}.workspace{flex:1;min-height:0;display:grid;overflow:auto;background:#e4eaf3}section{min-width:0;min-height:0;overflow:auto;background:white;position:relative}h2{font-size:14px;background:#253858;color:white;margin:-16px -44px 16px;padding:10px 44px}.form{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px}.form label,.triggers{display:grid;gap:6px;font-weight:600}.picker,.triggers>div{display:flex;gap:6px}input,select,button{min-height:34px;border:1px solid #bbc7d8;border-radius:5px;padding:6px 9px;background:white}input,select{width:100%}button{cursor:pointer;font-weight:600}.primary{background:#2563eb;color:white}.danger{background:#b42318;color:white}.success{background:#15803d;color:white}.secondary{background:#475569;color:white}.outline{color:#9a3412;border-color:#fb923c}.triggers{margin-top:16px}.triggers>div input{flex:1}details{margin-top:14px}summary{cursor:pointer;font-weight:600}.advanced{display:flex;gap:18px;align-items:center;padding:10px 0}.advanced label{display:flex;align-items:center;gap:7px}.advanced input[type=number]{width:100px}.advanced input[type=checkbox]{width:auto;min-height:auto}footer,.section-tools{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-top:16px}footer span{margin-left:auto;color:#475569}.section-tools{margin:0 0 12px;justify-content:flex-end}.table{overflow:auto;border:1px solid #dbe3ee;border-radius:6px}table{width:100%;min-width:720px;border-collapse:collapse}th,td{padding:7px;border:1px solid #dbe3ee;text-align:left}th{background:#edf2f9}.hint{margin-bottom:12px}.hidden{display:none}.splitter{height:8px;background:#d5deeb;border-top:1px solid #b9c6d8;border-bottom:1px solid #b9c6d8;position:relative;cursor:row-resize;touch-action:none}.splitter:after{content:'⋮';position:absolute;left:50%;top:-7px;color:#64748b;font-size:18px;transform:rotate(90deg)}.progress{height:10px;background:#e4eaf3;border-radius:6px;overflow:hidden;margin-top:12px}.progress span{display:block;height:100%;background:#2563eb;transition:width .2s}.progress-text{margin-top:5px}.log{padding-bottom:0}.log pre{margin:0 -44px;padding:14px 44px;min-height:180px;background:#111827;color:#d1fae5;white-space:pre-wrap}@media(max-width:900px){main{height:auto;min-height:100vh;overflow:visible}.workspace{display:block!important;overflow:visible}.splitter{display:none}.connection,.transfer,.log{overflow:visible}.form{grid-template-columns:1fr}header{align-items:flex-start;gap:10px;flex-direction:column}h2{margin-left:-16px;margin-right:-16px;padding-left:16px;padding-right:16px}.advanced{align-items:flex-start;flex-direction:column}.log pre{margin:0 -16px;padding:14px 16px}}
</style>
