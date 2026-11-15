import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { copyFile, open as openFile, stat } from "@tauri-apps/plugin-fs";
import { check } from "@tauri-apps/plugin-updater";
import { getLocale, setLocale, t, type Locale } from "./i18n";
import "./style.css";

type Device = { id: string; name: string; ip: string; port: number; lastSeen: number; paired: boolean };
type DeviceInfo = { id: string; name: string };
type PairRequest = { requestId: string; deviceId: string; deviceName: string; address: string };
type FileInfo = { name: string; size: number };
type TransferRequest = { transferId: string; deviceId: string; deviceName: string; files: FileInfo[]; totalBytes: number };
type PairedDevice = { id: string; name: string };
type ReceivedFile = { name: string; size: number; path: string; modified: number };
type TransferProgress = {
  transferId: string;
  direction: "sending" | "receiving";
  deviceName: string;
  fileName: string;
  fileIndex: number;
  fileCount: number;
  transferredBytes: number;
  totalBytes: number;
  status: "active" | "completed" | "failed";
};

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const deviceList = $("device-list");
const pairList = $("pair-list");
const transferList = $("transfer-list");
const pairedList = $("paired-list");
const receivedList = $("received-list");
const activeTransfersList = $("active-transfers-list");
const activeTransfers = new Map<string, TransferProgress>();
let devices: Device[] = [];
let paired: PairedDevice[] = [];
let pairRequests: PairRequest[] = [];
let transferRequests: TransferRequest[] = [];
let receivedFiles: ReceivedFile[] = [];
let busy = false;

const htmlEntities: Record<string, string> = {
  "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
};

function escapeHtml(value: string): string {
  return value.replace(/[&<>"']/g, (character) => htmlEntities[character] ?? character);
}

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return t("unknownSize");
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
    $("network-label").textContent = t("networkScanning");
    render();
  } catch (error) {
    $("network-label").textContent = t("networkFailed");
    toast(String(error), true);
  }
}

function renderDevices(): void {
  $("device-count").textContent = t("deviceCount", { count: devices.length });
  if (!devices.length) {
    deviceList.innerHTML = `<div class="empty-state"><span class="empty-icon">⌁</span><strong>${t("noDevices")}</strong><p>${t("noDevicesHint")}</p></div>`;
    return;
  }
  deviceList.innerHTML = devices.map((device) => {
    const isPaired = paired.some((peer) => peer.id === device.id);
    return `<article class="device-row">
      <div class="avatar">${escapeHtml(device.name.slice(0, 1).toUpperCase())}</div>
      <div class="device-details"><strong>${escapeHtml(device.name)}</strong><span>${escapeHtml(device.ip)} · ${isPaired ? t("paired") : t("newDevice")}</span></div>
      <button class="button ${isPaired ? "button-primary" : "button-secondary"}" data-action="${isPaired ? "send" : "pair"}" data-id="${escapeHtml(device.id)}">${isPaired ? t("sendFiles") : t("pair")}</button>
    </article>`;
  }).join("");
}

function renderPairs(): void {
  $("pair-count").textContent = t("pending", { count: pairRequests.length });
  pairList.innerHTML = pairRequests.length ? pairRequests.map((request) => `
    <article class="request-row">
      <div class="request-copy"><strong>${escapeHtml(request.deviceName)}</strong><span>${escapeHtml(request.address)} · ${t("wantsPair")}</span></div>
      <div class="request-actions"><button class="button button-quiet" data-action="decline-pair" data-id="${escapeHtml(request.requestId)}">${t("decline")}</button><button class="button button-primary" data-action="accept-pair" data-id="${escapeHtml(request.requestId)}">${t("accept")}</button></div>
    </article>`).join("") : `<p class="empty-inline">${t("noPairRequests")}</p>`;
}

function renderTransfers(): void {
  $("transfer-count").textContent = t("pending", { count: transferRequests.length });
  transferList.innerHTML = transferRequests.length ? transferRequests.map((request) => {
    const files = request.files.map((file) => `<li><span>${escapeHtml(file.name)}</span><span>${formatBytes(file.size)}</span></li>`).join("");
    return `<article class="transfer-card">
      <div class="request-copy"><strong>${escapeHtml(request.deviceName)} ${t("wantsFiles")}</strong><span>${t("files", { count: request.files.length })} · ${t("total", { size: formatBytes(request.totalBytes) })}</span></div>
      <ul class="file-preview">${files}</ul>
      <div class="request-actions"><button class="button button-quiet" data-action="decline-transfer" data-id="${escapeHtml(request.transferId)}">${t("decline")}</button><button class="button button-primary" data-action="accept-transfer" data-id="${escapeHtml(request.transferId)}">${t("acceptFiles")}</button></div>
    </article>`;
  }).join("") : `<p class="empty-inline">${t("noTransfers")}</p>`;
}

function updateTransferProgress(progress: TransferProgress): void {
  activeTransfers.set(progress.transferId, progress);
  renderActiveTransfers();

  if (progress.status !== "active") {
    window.setTimeout(() => {
      if (activeTransfers.get(progress.transferId) === progress) {
        activeTransfers.delete(progress.transferId);
        renderActiveTransfers();
      }
    }, 6000);
  }
}

function renderActiveTransfers(): void {
  const items = Array.from(activeTransfers.values());
  if (!items.length) {
    activeTransfersList.innerHTML = `<p class="empty-inline">${t("noActiveTransfers")}</p>`;
    return;
  }

  activeTransfersList.innerHTML = items.map((item) => {
    const percent = item.totalBytes > 0
      ? Math.min(100, Math.max(0, Math.round((item.transferredBytes / item.totalBytes) * 100)))
      : item.status === "completed" ? 100 : 0;
    const stateLabel = item.status === "completed"
      ? t("completed")
      : item.status === "failed" ? t("failed") : item.direction === "sending" ? t("sending") : t("receiving");
    const stateClass = item.status === "failed" ? "progress-failed"
      : item.status === "completed" ? "progress-completed" : "";
    const directionLabel = item.direction === "sending" ? t("sent") : t("received");
    const details = item.status === "active"
      ? `${formatBytes(item.transferredBytes)} / ${formatBytes(item.totalBytes)} · ${percent}%`
      : item.status === "completed" ? t("transferred", { size: formatBytes(item.totalBytes) }) : t("transferFailed");

    return `<article class="active-transfer-card ${stateClass}">
      <div class="active-transfer-heading">
        <div class="request-copy"><strong>${escapeHtml(item.deviceName)}</strong><span>${directionLabel} · ${escapeHtml(item.fileName)} · ${t("fileProgress", { current: item.fileIndex + 1, total: item.fileCount })}</span></div>
        <span class="transfer-state">${stateLabel}</span>
      </div>
      <div class="progress-track"><div class="progress-fill" style="width:${percent}%"></div></div>
      <div class="progress-meta"><span>${details}</span><span>${escapeHtml(item.fileName)}</span></div>
    </article>`;
  }).join("");
}

function renderPaired(): void {
  $("paired-count").textContent = t("deviceCount", { count: paired.length });
  pairedList.innerHTML = paired.length ? paired.map((peer) => `
    <article class="paired-row"><div class="avatar avatar-small">${escapeHtml(peer.name.slice(0, 1).toUpperCase())}</div><strong>${escapeHtml(peer.name)}</strong><button class="text-button" data-action="unpair" data-id="${escapeHtml(peer.id)}">${t("removePair")}</button></article>`
  ).join("") : `<p class="empty-inline">${t("noPaired")}</p>`;
}

async function renderReceived(): Promise<void> {
  try {
    receivedFiles = await invoke<ReceivedFile[]>("list_received_files");
    receivedList.innerHTML = receivedFiles.length ? receivedFiles.map((file) => `
      <article class="received-row">
        <div class="file-icon">↧</div>
        <div class="device-details"><strong>${escapeHtml(file.name)}</strong><span>${formatBytes(file.size)} · ${new Date(Number(file.modified)).toLocaleString()}</span></div>
        <button class="button button-secondary" data-action="export" data-id="${escapeHtml(file.name)}">Dışa aktar</button>
      </article>`
    ).join("") : `<p class="empty-inline">${t("noReceived")}</p>`;
  } catch (error) {
    receivedList.innerHTML = `<p class="empty-inline">${t("readError", { error: escapeHtml(String(error)) })}</p>`;
  }
}

async function exportReceivedFile(name: string): Promise<void> {
  const file = receivedFiles.find((item) => item.name === name);
  if (!file) throw new Error(t("fileMissing"));

  const destination = await saveDialog({
    defaultPath: file.name,
    title: t("exportDialog"),
  });
  if (!destination) return;

  await copyFile(file.path, destination);
  toast(t("exported", { name: file.name }));
}

function render(): void {
  renderDevices();
  renderPairs();
  renderTransfers();
  renderActiveTransfers();
  renderPaired();
}

async function requestPair(deviceId: string): Promise<void> {
  if (busy) return;
  busy = true;
  try {
    toast(t("pairWait"));
    await invoke("request_pairing", { deviceId });
    toast(t("pairSuccess"));
    await refresh();
  } catch (error) {
    toast(t("pairFailed", { error: String(error) }), true);
  } finally {
    busy = false;
  }
}

async function sendTo(device: Device): Promise<void> {
  if (busy) return;
  const selected = await openDialog({ multiple: true, title: t("selectFiles") });
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
    if (!selections.length) throw new Error(t("noFiles"));

    const metadata = selections.map(({ name, size }) => ({ name, size }));
    const totalBytes = metadata.reduce((total, file) => total + file.size, 0);
    let transferredBytes = 0;
    toast(t("sendWait", { count: metadata.length }));
    transferId = await invoke<string>("start_transfer", { deviceId: device.id, files: metadata });
    updateTransferProgress({
      transferId,
      direction: "sending",
      deviceName: device.name,
      fileName: metadata[0].name,
      fileIndex: 0,
      fileCount: metadata.length,
      transferredBytes: 0,
      totalBytes,
      status: "active",
    });

    for (let fileIndex = 0; fileIndex < selections.length; fileIndex++) {
      const selection = selections[fileIndex];
      const handle = await openFile(selection.path, { read: true });
      const buffer = new Uint8Array(256 * 1024);
      let sent = 0;
      try {
        while (sent < selection.size) {
          const remaining = selection.size - sent;
          const chunk = buffer.subarray(0, Math.min(buffer.length, remaining));
          const count = await handle.read(chunk);
          if (count === null || count === 0) break;
          await invoke("write_file_chunk", {
            transferId,
            bytes: Array.from(chunk.subarray(0, count)),
          });
          sent += count;
          transferredBytes += count;
          updateTransferProgress({
            transferId,
            direction: "sending",
            deviceName: device.name,
            fileName: selection.name,
            fileIndex,
            fileCount: selections.length,
            transferredBytes,
            totalBytes,
            status: "active",
          });
        }
      } finally {
        await handle.close();
      }
      if (sent !== selection.size) throw new Error(`${selection.name} okunurken dosya boyutu değişti.`);
      await invoke("finish_file", { transferId, fileIndex });
    }

    await invoke("finish_transfer", { transferId });
    updateTransferProgress({
      transferId,
      direction: "sending",
      deviceName: device.name,
      fileName: selections[selections.length - 1].name,
      fileIndex: selections.length - 1,
      fileCount: selections.length,
      transferredBytes: totalBytes,
      totalBytes,
      status: "completed",
    });
    toast(t("sendSuccess"));
  } catch (error) {
    if (transferId) {
      await invoke("cancel_transfer", { transferId }).catch(() => undefined);
      const current = activeTransfers.get(transferId);
      if (current) updateTransferProgress({ ...current, status: "failed" });
    }
    toast(t("sendFailed", { error: String(error) }), true);
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
        toast(t("pairAccepted"));
        await refresh();
        break;
      case "decline-pair":
        await invoke("respond_to_pair_request", { requestId: id, accept: false });
        await refresh();
        break;
      case "accept-transfer":
        await invoke("respond_to_transfer", { transferId: id, accept: true });
        toast(t("receivingToast"));
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
      case "export":
        await exportReceivedFile(id);
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
  await listen<Device[]>("devices-updated", (event) => {
    devices = event.payload;
    renderDevices();
  });
  const reloadPairs = async () => {
    pairRequests = await invoke<PairRequest[]>("get_pair_requests");
    renderPairs();
  };
  const reloadTransfers = async () => {
    transferRequests = await invoke<TransferRequest[]>("get_transfer_requests");
    renderTransfers();
  };
  await listen("pair-request", () => { void reloadPairs(); });
  await listen("pair-queue-updated", () => { void reloadPairs(); });
  await listen("transfer-request", () => { void reloadTransfers(); });
  await listen("transfer-queue-updated", () => { void reloadTransfers(); });
  await listen<TransferProgress>("transfer-progress", (event) => {
    updateTransferProgress(event.payload);
  });
  await listen("file-received", () => { void renderReceived(); });
  await listen("network-error", (event) => {
    $("network-label").textContent = t("networkBlocked");
    toast(String(event.payload), true);
  });
  await refresh();
  await renderReceived();
  const updateButton = $("check-updates") as HTMLButtonElement;
  const canUpdate = await invoke<boolean>("updater_supported").catch(() => false);
  updateButton.hidden = !canUpdate;
}

const localeSelect = $("language-select") as HTMLSelectElement;
function applyLanguage(): void {
  const locale = getLocale();
  document.documentElement.lang = locale;
  document.querySelectorAll<HTMLElement>("[data-i18n]").forEach((element) => {
    const key = element.dataset.i18n;
    if (key) element.textContent = t(key);
  });
  document.querySelectorAll<HTMLElement>("[data-i18n-aria]").forEach((element) => {
    const key = element.dataset.i18nAria;
    if (key) element.setAttribute("aria-label", t(key));
  });
  localeSelect.value = locale;
}
applyLanguage();
localeSelect.addEventListener("change", () => {
  setLocale(localeSelect.value as Locale);
  applyLanguage();
  render();
  void renderReceived();
});
$("check-updates").addEventListener("click", async () => {
  const button = $("check-updates") as HTMLButtonElement;
  button.disabled = true;
  button.textContent = t("checkingUpdates");
  try {
    const update = await check();
    if (!update) { toast(t("upToDate")); return; }
    if (!window.confirm(t("updateAvailable", { version: update.version }))) return;
    await update.downloadAndInstall();
    toast(t("updateInstalled"));
    await invoke("restart_app");
  } catch (error) {
    toast(t("updateFailed", { error: String(error) }), true);
  } finally {
    button.disabled = false;
    button.textContent = t("checkUpdates");
  }
});
void start();
