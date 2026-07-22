/* Mikro Cari Kartı Aktarma — frontend.
   Tauri 2 + withGlobalTauri: window.__TAURI__.core.invoke / .event.listen */

const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;

const $ = (id) => document.getElementById(id);

let running = false;
let connectionOk = false;
let backupOk = false;

/* ------------------------------------------------------------------ */
/* Log                                                                 */
/* ------------------------------------------------------------------ */

function log(msg, kind = "info") {
  const line = document.createElement("div");
  line.className = "log-line log-" + kind;

  const time = document.createElement("span");
  time.className = "log-time";
  time.textContent = new Date().toLocaleTimeString("tr-TR");

  line.appendChild(time);
  line.appendChild(document.createTextNode(msg));

  const box = $("log");
  box.appendChild(line);
  box.scrollTop = box.scrollHeight;
}

/* ------------------------------------------------------------------ */
/* Modal                                                               */
/* ------------------------------------------------------------------ */

function showModal(title, body, { kind = "", buttons = [{ label: "Tamam" }] } = {}) {
  return new Promise((resolve) => {
    const head = $("modalHead");
    head.textContent = title;
    head.className = "modal-head" + (kind ? " " + kind : "");
    $("modalBody").textContent = body;

    const actions = $("modalActions");
    actions.textContent = "";

    buttons.forEach((b, i) => {
      const btn = document.createElement("button");
      btn.className = "btn " + (b.cls || (i === buttons.length - 1 ? "btn-blue" : ""));
      btn.textContent = b.label;
      btn.addEventListener("click", () => {
        $("modal").classList.add("hidden");
        resolve(b.value !== undefined ? b.value : true);
      });
      actions.appendChild(btn);
    });

    $("modal").classList.remove("hidden");
  });
}

/* ------------------------------------------------------------------ */
/* Grid                                                                */
/* ------------------------------------------------------------------ */

function addRow(eski = "", yeni = "", sil = true) {
  const tr = document.createElement("tr");

  const tdNo = document.createElement("td");
  tdNo.className = "center";
  const no = document.createElement("div");
  no.className = "rowno";
  tdNo.appendChild(no);

  const mkInput = (val, placeholder) => {
    const td = document.createElement("td");
    const inp = document.createElement("input");
    inp.type = "text";
    inp.className = "cell-input";
    inp.value = val;
    inp.placeholder = placeholder;
    inp.spellcheck = false;
    td.appendChild(inp);
    return { td, inp };
  };

  const eskiCell = mkInput(eski, "120.1.İNT.HB.1156");
  const yeniCell = mkInput(yeni, "ESK-120.1.İNT.HB.1156");

  const tdSil = document.createElement("td");
  tdSil.className = "center";
  const chk = document.createElement("input");
  chk.type = "checkbox";
  chk.checked = sil;
  chk.title = "İşaretli: eski kart silinir (yeniden adlandırma).\nİşaretsiz: eski kart kalır, yeni kart kopyalanır.";
  tdSil.appendChild(chk);

  const tdDurum = document.createElement("td");
  const status = document.createElement("span");
  status.className = "status idle";
  status.textContent = "—";
  tdDurum.appendChild(status);

  const tdDel = document.createElement("td");
  tdDel.className = "center";
  const del = document.createElement("button");
  del.className = "row-del";
  del.textContent = "✕";
  del.title = "Satırı sil";
  del.addEventListener("click", () => {
    if (running) return;
    tr.remove();
    renumber();
  });
  tdDel.appendChild(del);

  tr.append(tdNo, eskiCell.td, yeniCell.td, tdSil, tdDurum, tdDel);
  tr._refs = { no, eski: eskiCell.inp, yeni: yeniCell.inp, sil: chk, status };

  $("gridBody").appendChild(tr);
  renumber();
  return tr;
}

function renumber() {
  [...$("gridBody").children].forEach((tr, i) => {
    tr._refs.no.textContent = i + 1;
  });
}

function collectRows() {
  return [...$("gridBody").children]
    .map((tr) => ({
      eski: tr._refs.eski.value.trim(),
      yeni: tr._refs.yeni.value.trim(),
      sil: tr._refs.sil.checked,
    }))
    .filter((r) => r.eski !== "" || r.yeni !== "");
}

/** collectRows() boş satırları atar; grid indeksiyle eşleşen tr listesi lazım. */
function activeRowElements() {
  return [...$("gridBody").children].filter(
    (tr) => tr._refs.eski.value.trim() !== "" || tr._refs.yeni.value.trim() !== ""
  );
}

function setRowStatus(tr, kind, text) {
  if (!tr) return;
  tr._refs.status.className = "status " + kind;
  tr._refs.status.textContent = text;
  tr._refs.status.title = text;
}

function resetStatuses() {
  [...$("gridBody").children].forEach((tr) => setRowStatus(tr, "idle", "—"));
}

/* ------------------------------------------------------------------ */
/* CSV içe aktarma                                                     */
/* ------------------------------------------------------------------ */

function csvDelimiter(text) {
  const firstLine = text.replace(/^\uFEFF/, "").split(/\r?\n/, 1)[0] || "";
  const candidates = [";", ",", "\t"];
  return candidates.reduce(
    (best, delimiter) => (firstLine.split(delimiter).length > firstLine.split(best).length ? delimiter : best),
    ";"
  );
}

/** RFC 4180'deki çift tırnak kuralını destekleyen küçük CSV okuyucu. */
function parseCsv(text) {
  const delimiter = csvDelimiter(text);
  const rows = [];
  let row = [];
  let value = "";
  let quoted = false;

  for (let i = 0; i < text.length; i += 1) {
    const ch = text[i];
    if (ch === '"') {
      if (quoted && text[i + 1] === '"') {
        value += '"';
        i += 1;
      } else {
        quoted = !quoted;
      }
    } else if (!quoted && ch === delimiter) {
      row.push(value.trim());
      value = "";
    } else if (!quoted && (ch === "\n" || ch === "\r")) {
      if (ch === "\r" && text[i + 1] === "\n") i += 1;
      row.push(value.trim());
      if (row.some((cell) => cell !== "")) rows.push(row);
      row = [];
      value = "";
    } else {
      value += ch;
    }
  }

  row.push(value.trim());
  if (row.some((cell) => cell !== "")) rows.push(row);
  return rows;
}

function hasCsvHeader(row) {
  const first = (row[0] || "").toLocaleLowerCase("tr-TR");
  const second = (row[1] || "").toLocaleLowerCase("tr-TR");
  return first.includes("eski") || first.includes("kaynak") || second.includes("yeni") || second.includes("hedef");
}

function parseSil(value) {
  const normalized = String(value || "").trim().toLocaleLowerCase("tr-TR");
  return !["0", "hayır", "hayir", "false", "no", "kalsın", "kalsin"].includes(normalized);
}

async function importCsv(file) {
  const text = (await file.text()).replace(/^\uFEFF/, "");
  const parsed = parseCsv(text);
  const data = hasCsvHeader(parsed[0] || []) ? parsed.slice(1) : parsed;
  const rows = data.filter((row) => row[0] || row[1]);

  if (!rows.length) throw new Error("CSV içinde aktarılacak satır bulunamadı.");
  const incomplete = rows.find((row) => !row[0] || !row[1]);
  if (incomplete) throw new Error("CSV'de eski veya yeni cari kodu boş olan satır var.");

  const onlyEmptyRow = $("gridBody").children.length === 1 &&
    !$("gridBody").children[0]._refs.eski.value &&
    !$("gridBody").children[0]._refs.yeni.value;
  if (onlyEmptyRow) $("gridBody").textContent = "";

  rows.forEach((row) => addRow(row[0], row[1], parseSil(row[2])));
  log(`${rows.length} satır CSV'den içeri aktarıldı: ${file.name}`, "ok");
  await showModal("CSV İçeri Aktarıldı", `${rows.length} satır eklendi. Aktarımı başlatmadan önce listeyi kontrol edin.`, { kind: "success" });
}

/* ------------------------------------------------------------------ */
/* Config                                                              */
/* ------------------------------------------------------------------ */

function authMode() {
  return document.querySelector('input[name="auth"]:checked').value;
}

function dbConfig() {
  return {
    server: $("server").value.trim(),
    database: $("database").value.trim(),
    auth: authMode(),
    username: $("username").value,
    password: $("password").value,
    trustCert: true,
  };
}

function triggerCfg() {
  return {
    name: $("trigger").value.trim(),
    table: $("triggerTable").value.trim(),
  };
}

function syncAuthFields() {
  const isSql = authMode() === "sql";
  document.querySelectorAll(".sqlonly").forEach((el) => {
    el.classList.toggle("hidden-field", !isSql);
  });
}

function setConnState(text, cls) {
  const el = $("connState");
  el.textContent = text;
  el.className = "conn-state" + (cls ? " " + cls : "");
}

/** Bağlantı ayarı değişince testi geçersiz kıl. */
function invalidateConnection() {
  if (running) return;
  connectionOk = false;
  backupOk = false;
  $("btnRun").disabled = true;
  $("btnBackup").disabled = true;
  setConnState("Ayar değişti — tekrar test edin", "");
}

/* ------------------------------------------------------------------ */
/* Progress                                                            */
/* ------------------------------------------------------------------ */

function setProgress(done, total) {
  const pct = total ? Math.round((done / total) * 100) : 0;
  $("progressBar").style.width = pct + "%";
  $("progressText").textContent = `${done} / ${total} satır (%${pct})`;
}

/* ------------------------------------------------------------------ */
/* Komutlar                                                            */
/* ------------------------------------------------------------------ */

async function testConnection() {
  $("btnTest").disabled = true;
  setConnState("Bağlanılıyor…", "");
  log(`Bağlantı test ediliyor: ${$("server").value} / ${$("database").value}`);

  try {
    const info = await invoke("test_connection", { cfg: dbConfig() });
    connectionOk = true;
    backupOk = false;
    $("btnBackup").disabled = false;
    $("btnRun").disabled = true;
    setConnState("Bağlantı hazır — yedek alınmalı", "ok");
    log("Bağlantı başarılı.", "ok");
    await showModal("Bağlantı Testi", info, { kind: "success" });
  } catch (e) {
    connectionOk = false;
    backupOk = false;
    $("btnBackup").disabled = true;
    $("btnRun").disabled = true;
    setConnState("Bağlantı başarısız", "bad");
    log("Bağlantı hatası: " + e, "error");
    await showModal("Bağlantı Hatası", String(e), { kind: "danger" });
  } finally {
    $("btnTest").disabled = false;
  }
}

async function takeBackup() {
  const directory = $("backupDirectory").value.trim();
  if (!directory) {
    await showModal("Yedek Klasörü", "SQL Server'ın erişebildiği yedek klasörünü girin.", { kind: "danger" });
    return;
  }

  const confirm = await showModal(
    "Yedek Al",
    `SQL Server üzerinde tam COPY_ONLY yedek alınacak.\n\nKlasör: ${directory}\n\nBu klasöre SQL Server hizmet hesabının yazma yetkisi olmalıdır.`,
    { buttons: [{ label: "Vazgeç", value: false }, { label: "Yedeği Al", value: true, cls: "btn-red" }] }
  );
  if (!confirm) return;

  $("btnBackup").disabled = true;
  setConnState("Yedek alınıyor…", "");
  log(`Ön yedek başlatıldı: ${directory}`, "warn");
  try {
    const result = await invoke("backup_database", { cfg: dbConfig(), backupDirectory: directory });
    backupOk = true;
    $("btnRun").disabled = false;
    setConnState("Yedek hazır — aktarım yapılabilir", "ok");
    log(result.message, "ok");
    await showModal("Yedek Tamamlandı", result.message, { kind: "success" });
  } catch (e) {
    backupOk = false;
    $("btnRun").disabled = true;
    setConnState("Yedek alınamadı", "bad");
    log("Yedek hatası: " + e, "error");
    await showModal("Yedek Hatası", String(e), { kind: "danger" });
  } finally {
    $("btnBackup").disabled = !connectionOk;
  }
}

async function checkTrigger() {
  $("btnTrigger").disabled = true;
  try {
    const info = await invoke("trigger_status", {
      cfg: dbConfig(),
      trigger: triggerCfg(),
    });
    log(info, info.includes("DEVRE DIŞI") ? "warn" : "info");
    await showModal("Trigger Durumu", info);
  } catch (e) {
    log("Trigger durumu alınamadı: " + e, "error");
    await showModal("Hata", String(e), { kind: "danger" });
  } finally {
    $("btnTrigger").disabled = false;
  }
}

async function enableTriggerManually() {
  $("btnEnable").disabled = true;
  try {
    const info = await invoke("enable_trigger", {
      cfg: dbConfig(),
      trigger: triggerCfg(),
    });
    log(info, "ok");
    await showModal("Trigger", info, { kind: "success" });
  } catch (e) {
    log("Trigger açılamadı: " + e, "error");
    await showModal("Hata", String(e), { kind: "danger" });
  } finally {
    $("btnEnable").disabled = false;
  }
}

async function runTransfer() {
  const rows = collectRows();

  if (rows.length === 0) {
    await showModal("Uyarı", "Aktarılacak satır yok.", { kind: "danger" });
    return;
  }

  const bad = rows.find((r) => !r.eski || !r.yeni);
  if (bad) {
    await showModal("Uyarı", "Her satırda hem eski hem yeni kod dolu olmalı.", { kind: "danger" });
    return;
  }

  const silCount = rows.filter((r) => r.sil).length;
  const confirmText =
    `${rows.length} satır aktarılacak.\n` +
    `  • ${silCount} satırda eski kart SİLİNECEK (yeniden adlandırma)\n` +
    `  • ${rows.length - silCount} satırda eski kart korunacak (kopyalama)\n\n` +
    `Trigger: ${$("trigger").value || "(yönetilmeyecek)"}\n` +
    `Veritabanı: ${$("database").value} @ ${$("server").value}\n\n` +
    `Bu işlem GERİ ALINAMAZ. Yedeğinizi aldınız mı?\n` +
    `Mikro uygulamasının kapalı olduğundan emin olun.`;

  const go = await showModal("Aktarımı Onayla", confirmText, {
    kind: "danger",
    buttons: [
      { label: "Vazgeç", value: false },
      { label: "Evet, Aktarımı Başlat", value: true, cls: "btn-green" },
    ],
  });
  if (!go) {
    log("Aktarım kullanıcı tarafından iptal edildi (onay verilmedi).", "warn");
    return;
  }

  running = true;
  resetStatuses();
  setProgress(0, rows.length);
  $("btnRun").disabled = true;
  $("btnTest").disabled = true;
  $("btnCancel").disabled = false;
  log(`Aktarım başlatıldı — ${rows.length} satır.`, "warn");

  try {
    const summary = await invoke("run_transfer", {
      cfg: dbConfig(),
      trigger: triggerCfg(),
      rows,
      cariTipi: parseInt($("cariTipi").value, 10),
      userId: parseInt($("userId").value, 10) || 0,
      sonDegGuncelle: $("sonDeg").checked,
    });

    log(`Aktarım bitti — ${summary.ok} başarılı, ${summary.failed} hatalı.`,
        summary.failed ? "warn" : "ok");

    let body =
      `Toplam : ${summary.total}\n` +
      `Başarılı: ${summary.ok}\n` +
      `Hatalı  : ${summary.failed}\n\n` +
      `Trigger: ${summary.triggerMessage}`;

    if (summary.errors.length) {
      body += "\n\nHatalar:\n" + summary.errors.map((e) => "  • " + e).join("\n");
    }

    if (!summary.triggerRestored) {
      log(summary.triggerMessage, "error");
      await showModal("⚠ TRIGGER GERİ AÇILAMADI", body, { kind: "danger" });
    } else {
      await showModal("Aktarım Tamamlandı", body, {
        kind: summary.failed ? "danger" : "success",
      });
    }
  } catch (e) {
    log("Aktarım başarısız: " + e, "error");
    await showModal("Aktarım Hatası", String(e), { kind: "danger" });
  } finally {
    running = false;
    $("btnRun").disabled = !connectionOk || !backupOk;
    $("btnTest").disabled = false;
    $("btnCancel").disabled = true;
  }
}

async function cancelTransfer() {
  $("btnCancel").disabled = true;
  log("İptal istendi — devam eden satır bitince duracak.", "warn");
  await invoke("cancel_transfer");
}

/* ------------------------------------------------------------------ */
/* Event listeners (Rust -> UI)                                        */
/* ------------------------------------------------------------------ */

async function wireEvents() {
  await listen("row-status", (ev) => {
    const p = ev.payload;
    const tr = activeRowElements()[p.index];
    const icon = { running: "⏳", ok: "✓", error: "✗" }[p.status] || "";
    setRowStatus(tr, p.status, `${icon} ${p.message}`);

    if (p.status === "ok") log(`✓ ${p.eski} → ${p.yeni} — ${p.message}`, "ok");
    if (p.status === "error") log(`✗ ${p.eski} → ${p.yeni} — ${p.message}`, "error");
  });

  await listen("progress", (ev) => setProgress(ev.payload.done, ev.payload.total));

  await listen("log", (ev) => log(String(ev.payload), "info"));

  await listen("trigger-alert", (ev) => log(String(ev.payload), "error"));
}

/* ------------------------------------------------------------------ */
/* Init                                                                */
/* ------------------------------------------------------------------ */

function init() {
  addRow();
  syncAuthFields();

  $("btnAddRow").addEventListener("click", () => addRow());
  $("btnImportCsv").addEventListener("click", () => {
    if (!running) $("csvFile").click();
  });
  $("csvFile").addEventListener("change", async (event) => {
    const [file] = event.target.files;
    event.target.value = "";
    if (!file || running) return;
    try {
      await importCsv(file);
    } catch (e) {
      log("CSV içeri aktarılamadı: " + e.message, "error");
      await showModal("CSV Hatası", e.message, { kind: "danger" });
    }
  });
  $("btnClear").addEventListener("click", () => {
    if (running) return;
    $("gridBody").textContent = "";
    addRow();
    setProgress(0, 0);
    $("progressText").textContent = "Hazır";
  });

  $("btnTest").addEventListener("click", testConnection);
  $("btnBackup").addEventListener("click", takeBackup);
  $("btnTrigger").addEventListener("click", checkTrigger);
  $("btnEnable").addEventListener("click", enableTriggerManually);
  $("btnRun").addEventListener("click", runTransfer);
  $("btnCancel").addEventListener("click", cancelTransfer);

  $("btnClearLog").addEventListener("click", () => ($("log").textContent = ""));
  $("btnCopyLog").addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText($("log").innerText);
      log("Günlük panoya kopyalandı.", "ok");
    } catch {
      log("Panoya kopyalanamadı.", "error");
    }
  });

  document.querySelectorAll('input[name="auth"]').forEach((r) =>
    r.addEventListener("change", () => {
      syncAuthFields();
      invalidateConnection();
    })
  );

  ["server", "database", "username", "password", "backupDirectory"].forEach((id) =>
    $(id).addEventListener("input", invalidateConnection)
  );

  wireEvents();
  log("Uygulama hazır. Önce bağlantıyı test edin.");
}

window.addEventListener("DOMContentLoaded", init);
