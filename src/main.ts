import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { open as openFile, stat } from "@tauri-apps/plugin-fs";
import "./style.css";

type Device = { id: string; name: string; ip: string; port: number; lastSeen: number; paired: boolean };
type DeviceInfo = { id: string; name: string };
type PairRequest = { requestId: string; deviceId: string; deviceName: string; address: string };
type FileInfo = { name: string; size: number };
type TransferRequest = { transferId: string; deviceId: string; deviceName: string; files: FileInfo[]; totalBytes: number };
type PairedDevice = { id: string; name: string };
type ReceivedFile = { name: string; size: number; path: string; modified: number };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const deviceList = $("device-list");
const pairList = $("pair-list");
const transferList = $("transfer-list");
const pairedList = $("paired-list");
const receivedList = $("received-list");
let devices: Device[] = [];
let paired: PairedDevice[] = [];
let pairRequests: PairRequest[] = [];
let transferRequests: TransferRequest[] = [];
let busy = false;

const htmlEntities: Record<string, string> = {
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
};

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => htmlEntities[character] ?? character);
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "Bilinmeyen boyut";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let index = 0;
  while (value >= 1024 && index < units.length - 1) { value /= 1024; index++; }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[index]}`;
}

function toast(message: string, error = false): void {
  const item = document.createElement("div");
  item.className = `toast${error ? " toast-error" : ""}`;
  item.textContent = message;
  $("toast-region").append(item);
  window.setTimeout(() => item.remove(), 4200);
}

async function refresh(): Promise<void> {
  try {
    const [info, nearby, pairQueue, transferQueue, pairedDevices] = await Promise.all([
      invoke<DeviceInfo>("get_device_info"),
      invoke<Device[]>("get_discovered_devices"),
      invoke<PairRequest[]>("get_pair_requests"),
      invoke<TransferRequest[]>("get_transfer_requests"),
      invoke<PairedDevice[]>("get_paired_devices"),
    ]);
    $("device-name").textContent = info.name;
    $("device-id").textContent = info.id;
    devices = nearby;
    pairRequests = pairQueue;
    transferRequests = transferQueue;
    paired = pairedDevices;
    $("network-label").textContent = "Yerel ağ taranıyor";
    render();
  } catch (error) {
    $("network-label").textContent = "Ağ servisi başlatılamadı";
    toast(String(error), true);
  }
}

function renderDevices(): void {
  $("device-count").textContent = `${devices.length} cihaz`;
  if (!devices.length) {
    deviceList.innerHTML = `<div class="empty-state"><span class="empty-icon">⌁</span><strong>Henüz cihaz bulunamadı</strong><p>Diğer cihazlarda Fileway'in açık olduğundan ve aynı Wi-Fi/Ethernet ağına bağlı olduğundan emin ol.</p></div>`;
    return;
  }
  deviceList.innerHTML = devices.map((device) => {
    const isPaired = paired.some((peer) => peer.id === device.id);
    return `<article class="device-row">
      <div class="avatar">${escapeHtml(device.name.slice(0, 1).toUpperCase())}</div>
      <div class="device-details"><strong>${escapeHtml(device.name)}</strong><span>${escapeHtml(device.ip)} · ${isPaired ? "Eşleşmiş" : "Yeni cihaz"}</span></div>
      <button class="button ${isPaired ? "button-primary" : "button-secondary"}" data-action="${isPaired ? "send" : "pair"}" data-id="${escapeHtml(device.id)}">${isPaired ? "Dosya gönder" : "Eşleş"}</button>
    </article>`;
  }).join("");
}

function renderPairs(): void {
  $("pair-count").textContent = `${pairRequests.length} bekliyor`;
  pairList.innerHTML = pairRequests.length ? pairRequests.map((request) => `
    <article class="request-row">
      <div class="request-copy"><strong>${escapeHtml(request.deviceName)}</strong><span>${escapeHtml(request.address)} · eşleşmek istiyor</span></div>
      <div class="request-actions"><button class="button button-quiet" data-action="decline-pair" data-id="${escapeHtml(request.requestId)}">Reddet</button><button class="button button-primary" data-action="accept-pair" data-id="${escapeHtml(request.requestId)}">Kabul et</button></div>
    </article>`).join("") : '<p class="empty-inline">Bekleyen eşleşme isteği yok.</p>';
}

function renderTransfers(): void {
  $("transfer-count").textContent = `${transferRequests.length} bekliyor`;
  transferList.innerHTML = transferRequests.length ? transferRequests.map((request) => {
    const files = request.files.map((file) => `<li><span>${escapeHtml(file.name)}</span><span>${formatBytes(file.size)}</span></li>`).join("");
    return `<article class="transfer-card">
      <div class="request-copy"><strong>${escapeHtml(request.deviceName)} dosya göndermek istiyor</strong><span>${request.files.length} dosya · toplam ${formatBytes(request.totalBytes)}</span></div>
      <ul class="file-preview">${files}</ul>
      <div class="request-actions"><button class="button button-quiet" data-action="decline-transfer" data-id="${escapeHtml(request.transferId)}">Reddet</button><button class="button button-primary" data-action="accept-transfer" data-id="${escapeHtml(request.transferId)}">Dosyaları kabul et</button></div>
    </article>`;
  }).join("") : '<p class="empty-inline">Bekleyen dosya aktarımı yok.</p>';
}

function renderPaired(): void {
  $("paired-count").textContent = `${paired.length} cihaz`;
  pairedList.innerHTML = paired.length ? paired.map((peer) => `
    <article class="paired-row"><div class="avatar avatar-small">${escapeHtml(peer.name.slice(0, 1).toUpperCase())}</div><strong>${escapeHtml(peer.name)}</strong><button class="text-button" data-action="unpair" data-id="${escapeHtml(peer.id)}">Eşleşmeyi kaldır</button></article>`
  ).join("") : '<p class="empty-inline">Henüz eşleşmiş cihaz yok.</p>';
}

async function renderReceived(): Promise<void> {
  try {
    const files = await invoke<ReceivedFile[]>("list_received_files");
    receivedList.innerHTML = files.length ? files.map((file) => `
      <article class="received-row"><div class="file-icon">↧</div><div class="device-details"><strong>${escapeHtml(file.name)}</strong><span>${formatBytes(file.size)} · ${new Date(Number(file.modified)).toLocaleString()}</span></div></article>`
    ).join("") : '<p class="empty-inline">Henüz dosya alınmadı.</p>';
  } catch (error) {
    receivedList.innerHTML = `<p class="empty-inline">Dosyalar okunamadı: ${escapeHtml(String(error))}</p>`;
  }
}

function render(): void {
  renderDevices();
  renderPairs();
  renderTransfers();
  renderPaired();
  void renderReceived();
}

async function requestPair(deviceId: string): Promise<void> {
  if (busy) return;
  busy = true;
  try {
    toast("Eşleşme isteği gönderildi. Karşı cihazın onayı bekleniyor.");
    await invoke("request_pairing", { deviceId });
    toast("Cihazlar eşleştirildi.");
    await refresh();
  } catch (error) {
    toast(`Eşleşme başarısız: ${String(error)}`, true);
  } finally {
    busy = false;
  }
}

async function sendTo(device: Device): Promise<void> {
  if (busy) return;
  const selected = await openDialog({ multiple: true, title: "Gönderilecek dosyaları seç" });
  if (!selected) return;
  const paths = Array.isArray(selected) ? selected : [selected];
  if (!paths.length) return;

  busy = true;
  let transferId: string | null = null;
  try {
    const selections: Array<{ path: string; name: string; size: number }> = [];
    for (const path of paths) {
      const info = await stat(path);
      if (!info.isFile) continue;
      const rawName = path.split(/[\\/]/).pop() ?? "file";
      let name = rawName;
      try { name = decodeURIComponent(rawName); } catch { /* Keep the platform-provided name. */ }
      selections.push({ path, name, size: Number(info.size) });
    }
    if (!selections.length) throw new Error("Seçilen dosya bulunamadı.");

    const metadata = selections.map(({ name, size }) => ({ name, size }));
    toast(`${metadata.length} dosya için karşı cihazın onayı bekleniyor.`);
    transferId = await invoke<string>("start_transfer", { deviceId: device.id, files: metadata });

    for (let fileIndex = 0; fileIndex < selections.length; fileIndex++) {
      const selection = selections[fileIndex];
      const handle = await openFile(selection.path, { read: true });
      const buffer = new Uint8Array(256 * 1024);
      let sent = 0;
      try {
        while (true) {
          const count = await handle.read(buffer);
          if (count === null || count === 0) break;
          await invoke("write_file_chunk", {
            transferId,
            bytes: Array.from(buffer.subarray(0, count)),
          });
          sent += count;
        }
      } finally {
        await handle.close();
      }
      if (sent !== selection.size) throw new Error(`${selection.name} okunurken dosya boyutu değişti.`);
      await invoke("finish_file", { transferId, fileIndex });
    }

    await invoke("finish_transfer", { transferId });
    toast("Dosyalar başarıyla gönderildi.");
  } catch (error) {
    if (transferId) await invoke("cancel_transfer", { transferId }).catch(() => undefined);
    toast(`Aktarım tamamlanamadı: ${String(error)}`, true);
  } finally {
    busy = false;
    await refresh();
  }
}

async function handleAction(action: string, id: string): Promise<void> {
  try {
    switch (action) {
      case "pair":
        await requestPair(id);
        break;
      case "send": {
        const device = devices.find((item) => item.id === id);
        if (device) await sendTo(device);
        break;
      }
      case "accept-pair":
        await invoke("respond_to_pair_request", { requestId: id, accept: true });
        toast("Eşleşme kabul edildi.");
        await refresh();
        break;
      case "decline-pair":
        await invoke("respond_to_pair_request", { requestId: id, accept: false });
        await refresh();
        break;
      case "accept-transfer":
        await invoke("respond_to_transfer", { transferId: id, accept: true });
        toast("Dosyalar alınıyor.");
        await refresh();
        break;
      case "decline-transfer":
        await invoke("respond_to_transfer", { transferId: id, accept: false });
        await refresh();
        break;
      case "unpair":
        await invoke("remove_paired_device", { deviceId: id });
        await refresh();
        break;
    }
  } catch (error) {
    toast(String(error), true);
    await refresh();
  }
}

document.addEventListener("click", (event) => {
  const target = (event.target as HTMLElement).closest<HTMLElement>("[data-action]");
  if (target?.dataset.action && target.dataset.id) {
    void handleAction(target.dataset.action, target.dataset.id);
  }
});

$("scan-button").addEventListener("click", () => {
  void invoke("scan_now").then(refresh).catch((error) => toast(String(error), true));
});
$("refresh-received").addEventListener("click", () => { void renderReceived(); });

async function start(): Promise<void> {
  await listen("devices-updated", () => { void refresh(); });
  await listen("pair-request", () => { void refresh(); });
  await listen("transfer-request", () => { void refresh(); });
  await listen("file-received", () => { void renderReceived(); });
  await listen("network-error", (event) => {
    $("network-label").textContent = "Ağ erişimi engellendi";
    toast(String(event.payload), true);
  });
  await refresh();
  window.setInterval(() => { void refresh(); }, 3000);
}

void start();
