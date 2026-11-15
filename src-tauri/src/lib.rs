#[cfg(target_os = "ios")]
compile_error!("Fileway intentionally does not support iOS.");

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{oneshot, Mutex},
    time::{interval, timeout},
};
use uuid::Uuid;

const DISCOVERY_PORT: u16 = 39147;
const TRANSFER_PORT: u16 = 39148;
const PROTOCOL_VERSION: u8 = 1;
const BEACON_INTERVAL: Duration = Duration::from_secs(2);
const DEVICE_TTL: u64 = 12;
const MAX_HEADER_BYTES: usize = 256 * 1024;
const MAX_FILES_PER_TRANSFER: usize = 128;
const MAX_TRANSFER_BYTES: u64 = 100 * 1024 * 1024 * 1024;
const IO_CHUNK_BYTES: usize = 1024 * 1024;
const TRANSFER_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceInfo {
    id: String,
    name: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveredDevice {
    id: String,
    name: String,
    ip: String,
    port: u16,
    last_seen: u64,
    paired: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Beacon {
    kind: String,
    version: u8,
    device_id: String,
    device_name: String,
    transfer_port: u16,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairRequestView {
    request_id: String,
    device_id: String,
    device_name: String,
    address: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileInfo {
    name: String,
    size: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferRequestView {
    transfer_id: String,
    device_id: String,
    device_name: String,
    files: Vec<FileInfo>,
    total_bytes: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferProgress {
    transfer_id: String,
    direction: String,
    device_name: String,
    file_name: String,
    file_index: usize,
    file_count: usize,
    transferred_bytes: u64,
    total_bytes: u64,
    status: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PairedDeviceView {
    id: String,
    name: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceivedFile {
    name: String,
    size: u64,
    path: String,
    modified: u128,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairRecord {
    id: String,
    name: String,
    token: String,
}

#[derive(Serialize, Deserialize)]
struct StoredIdentity {
    id: String,
    name: String,
}

struct PairDecision {
    accepted: bool,
    token: Option<String>,
}

struct TransferDecision {
    accepted: bool,
}

struct AppState {
    app: AppHandle,
    data_dir: PathBuf,
    identity: DeviceInfo,
    devices: Mutex<HashMap<String, DiscoveredDevice>>,
    paired: Mutex<HashMap<String, PairRecord>>,
    pair_requests: Mutex<HashMap<String, PairRequestView>>,
    pair_waiters: Mutex<HashMap<String, oneshot::Sender<PairDecision>>>,
    transfer_requests: Mutex<HashMap<String, TransferRequestView>>,
    transfer_waiters: Mutex<HashMap<String, oneshot::Sender<TransferDecision>>>,
    outbound_streams: Mutex<HashMap<String, Arc<Mutex<BufReader<TcpStream>>>>>,
}

impl AppState {
    fn new(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = app.path().app_data_dir()?;
        std::fs::create_dir_all(&data_dir)?;

        let identity_path = data_dir.join("identity.json");
        let identity = if identity_path.exists() {
            let bytes = std::fs::read(&identity_path)?;
            serde_json::from_slice::<StoredIdentity>(&bytes)?
        } else {
            let name = hostname::get()
                .ok()
                .and_then(|value| value.into_string().ok())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "Fileway device".to_string());
            let identity = StoredIdentity {
                id: Uuid::new_v4().to_string(),
                name: name.chars().take(48).collect(),
            };
            std::fs::write(&identity_path, serde_json::to_vec_pretty(&identity)?)?;
            identity
        };

        let paired_path = data_dir.join("paired.json");
        let paired_records = if paired_path.exists() {
            std::fs::read(&paired_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Vec<PairRecord>>(&bytes).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let paired = paired_records
            .into_iter()
            .map(|record| (record.id.clone(), record))
            .collect();

        Ok(Self {
            app: app.clone(),
            data_dir,
            identity: DeviceInfo { id: identity.id, name: identity.name },
            devices: Mutex::new(HashMap::new()),
            paired: Mutex::new(paired),
            pair_requests: Mutex::new(HashMap::new()),
            pair_waiters: Mutex::new(HashMap::new()),
            transfer_requests: Mutex::new(HashMap::new()),
            transfer_waiters: Mutex::new(HashMap::new()),
            outbound_streams: Mutex::new(HashMap::new()),
        })
    }

    fn emit_transfer_progress(&self, progress: TransferProgress) {
        let _ = self.app.emit("transfer-progress", progress);
    }

    async fn save_paired(&self) -> Result<(), String> {
        let records = self.paired.lock().await.values().cloned().collect::<Vec<_>>();
        let encoded = serde_json::to_vec_pretty(&records).map_err(|error| error.to_string())?;
        fs::write(self.data_dir.join("paired.json"), encoded)
            .await
            .map_err(|error| error.to_string())
    }

    async fn paired_views(&self) -> Vec<PairedDeviceView> {
        let mut result = self.paired.lock().await.values()
            .map(|record| PairedDeviceView { id: record.id.clone(), name: record.name.clone() })
            .collect::<Vec<_>>();
        result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        result
    }

    async fn emit_devices(&self) {
        let paired = self.paired.lock().await;
        let now = unix_seconds();
        let mut devices = self.devices.lock().await;
        devices.retain(|_, item| now.saturating_sub(item.last_seen) <= DEVICE_TTL);
        let mut values = devices.values().cloned().map(|mut item| {
            item.paired = paired.contains_key(&item.id);
            item
        }).collect::<Vec<_>>();
        values.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let _ = self.app.emit("devices-updated", values);
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum IncomingMessage {
    PairRequest {
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "deviceId")]
        device_id: String,
        #[serde(rename = "deviceName")]
        device_name: String,
    },
    TransferRequest {
        #[serde(rename = "transferId")]
        transfer_id: String,
        #[serde(rename = "deviceId")]
        device_id: String,
        #[serde(rename = "deviceName")]
        device_name: String,
        token: String,
        files: Vec<FileInfo>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairResponse {
    kind: String,
    accepted: bool,
    token: Option<String>,
    message: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferResponse {
    kind: String,
    accepted: bool,
    message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileReceivedMessage {
    kind: &'static str,
    file_index: usize,
    ok: bool,
    message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferCompleteMessage {
    kind: &'static str,
    ok: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferAck {
    kind: String,
    file_index: Option<usize>,
    ok: bool,
    message: Option<String>,
}

#[derive(Deserialize)]
struct TransferCompleteAck {
    kind: String,
    ok: bool,
}

fn unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn sanitize_filename(input: &str) -> String {
    let leaf = input.rsplit(['/', '\\']).next().unwrap_or("file");
    let cleaned = leaf.chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(*character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
        })
        .collect::<String>();
    let cleaned = cleaned.trim().trim_matches(|character| character == '.' || character == ' ');
    if cleaned.is_empty() {
        return "file".to_string();
    }

    let windows_stem = cleaned.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    let reserved = matches!(windows_stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            windows_stem.strip_prefix(prefix)
                .and_then(|suffix| suffix.parse::<u8>().ok())
                .is_some_and(|number| (1..=9).contains(&number))
        });
    let safe_name = if reserved { format!("_{cleaned}") } else { cleaned.to_string() };
    safe_name.chars().take(180).collect()
}

fn unique_target(directory: &Path, original_name: &str) -> PathBuf {
    let name = sanitize_filename(original_name);
    let first = directory.join(&name);
    if !first.exists() { return first; }

    let path = Path::new(&name);
    let stem = path.file_stem().and_then(|value| value.to_str()).unwrap_or("file");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..10_000 {
        let candidate_name = match extension {
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None => format!("{stem} ({index})"),
        };
        let candidate = directory.join(candidate_name);
        if !candidate.exists() { return candidate; }
    }
    directory.join(format!("{}-{}", Uuid::new_v4(), name))
}

async fn write_json_line<T: Serialize>(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &T,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await.map_err(|error| error.to_string())
}

async fn read_json_line(reader: &mut BufReader<TcpStream>) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(256);
    loop {
        let byte = reader.read_u8().await
            .map_err(|error| format!("Bağlantıdan mesaj okunamadı: {error}"))?;
        if byte == b'\n' {
            break;
        }
        if bytes.len() >= MAX_HEADER_BYTES {
            return Err("Ağ mesajı izin verilen boyutu aşıyor.".to_string());
        }
        bytes.push(byte);
    }
    String::from_utf8(bytes).map_err(|_| "Ağ mesajı geçerli UTF-8 değil.".to_string())
}

fn is_same_local_subnet(peer: IpAddr) -> bool {
    match peer {
        IpAddr::V4(peer) if peer.is_loopback() => true,
        IpAddr::V4(peer) => {
            let peer_bits = u32::from(peer);
            if_addrs::get_if_addrs().map(|interfaces| {
                interfaces.into_iter().any(|interface| {
                    if let if_addrs::IfAddr::V4(address) = interface.addr {
                        if address.ip.is_loopback() || address.ip.is_unspecified() {
                            return false;
                        }
                        let mask = u32::from(address.netmask);
                        (peer_bits & mask) == (u32::from(address.ip) & mask)
                    } else {
                        false
                    }
                })
            }).unwrap_or(false)
        }
        IpAddr::V6(peer) => peer.is_loopback(),
    }
}

fn broadcast_targets() -> Vec<SocketAddr> {
    let mut targets = Vec::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if let if_addrs::IfAddr::V4(address) = interface.addr {
                if address.ip.is_loopback() || address.ip.is_unspecified() { continue; }
                let ip = u32::from(address.ip);
                let mask = u32::from(address.netmask);
                let broadcast = Ipv4Addr::from(ip | !mask);
                let target = SocketAddr::from((broadcast, DISCOVERY_PORT));
                if !targets.contains(&target) { targets.push(target); }
            }
        }
    }
    if targets.is_empty() {
        targets.push(SocketAddr::from(([255, 255, 255, 255], DISCOVERY_PORT)));
    }
    targets
}

async fn broadcast_beacon(socket: &UdpSocket, state: &AppState) {
    let message = Beacon {
        kind: "fileway_beacon".to_string(),
        version: PROTOCOL_VERSION,
        device_id: state.identity.id.clone(),
        device_name: state.identity.name.clone(),
        transfer_port: TRANSFER_PORT,
    };
    let Ok(payload) = serde_json::to_vec(&message) else { return; };
    for target in broadcast_targets() {
        let _ = socket.send_to(&payload, target).await;
    }
}

fn start_discovery(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let socket = match UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT)).await {
            Ok(socket) => socket,
            Err(error) => {
                eprintln!("Fileway discovery socket could not start: {error}");
                let _ = state.app.emit("network-error", error.to_string());
                return;
            }
        };
        if let Err(error) = socket.set_broadcast(true) {
            eprintln!("Fileway broadcast could not be enabled: {error}");
            return;
        }

        let mut ticker = interval(BEACON_INTERVAL);
        let mut buffer = vec![0u8; 2048];
        loop {
            tokio::select! {
                _ = ticker.tick() => broadcast_beacon(&socket, &state).await,
                received = socket.recv_from(&mut buffer) => {
                    let Ok((length, source)) = received else { continue; };
                    let Ok(beacon) = serde_json::from_slice::<Beacon>(&buffer[..length]) else { continue; };
                    if beacon.kind != "fileway_beacon"
                        || beacon.version != PROTOCOL_VERSION
                        || beacon.device_id == state.identity.id
                        || beacon.device_id.len() > 80
                        || beacon.device_name.trim().is_empty()
                        || beacon.transfer_port == 0
                    { continue; }

                    let record = DiscoveredDevice {
                        id: beacon.device_id,
                        name: beacon.device_name.chars().take(48).collect(),
                        ip: source.ip().to_string(),
                        port: beacon.transfer_port,
                        last_seen: unix_seconds(),
                        paired: false,
                    };
                    state.devices.lock().await.insert(record.id.clone(), record);
                    state.emit_devices().await;
                }
            }
        }
    });
}

fn start_transfer_server(state: Arc<AppState>) {
    tauri::async_runtime::spawn(async move {
        let listener = match TcpListener::bind(("0.0.0.0", TRANSFER_PORT)).await {
            Ok(listener) => listener,
            Err(error) => {
                eprintln!("Fileway transfer listener could not start: {error}");
                let _ = state.app.emit("network-error", error.to_string());
                return;
            }
        };
        loop {
            match listener.accept().await {
                Ok((stream, address)) => {
                    let state = state.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = handle_connection(stream, address, state).await {
                            eprintln!("Fileway connection error: {error}");
                        }
                    });
                }
                Err(error) => eprintln!("Fileway accept error: {error}"),
            }
        }
    });
}

async fn handle_connection(
    stream: TcpStream,
    address: SocketAddr,
    state: Arc<AppState>,
) -> Result<(), String> {
    if !is_same_local_subnet(address.ip()) {
        return Err("Fileway yalnızca aynı yerel ağdaki cihazlardan bağlantı kabul eder.".to_string());
    }
    let mut reader = BufReader::new(stream);
    let line = timeout(Duration::from_secs(30), read_json_line(&mut reader))
        .await.map_err(|_| "İlk ağ mesajı zaman aşımına uğradı.".to_string())??;
    let message: IncomingMessage = serde_json::from_str(&line).map_err(|_| "Geçersiz Fileway mesajı.".to_string())?;

    match message {
        IncomingMessage::PairRequest { request_id, device_id, device_name } => {
            if request_id.len() > 80 || device_id.len() > 80 || device_name.trim().is_empty() || device_name.len() > 96 {
                return Err("Eşleşme isteği geçersiz.".to_string());
            }
            if device_id == state.identity.id {
                return Err("Aynı cihazla eşleşilemez.".to_string());
            }
            if state.paired.lock().await.contains_key(&device_id) {
                write_json_line(reader.get_mut(), &PairResponse {
                    kind: "pair_response".to_string(), accepted: true, token: None, message: Some("Bu cihaz zaten eşleşmiş.".to_string())
                }).await?;
                return Ok(());
            }

            let view = PairRequestView {
                request_id: request_id.clone(),
                device_id,
                device_name: device_name.chars().take(48).collect(),
                address: address.ip().to_string(),
            };
            let (sender, receiver) = oneshot::channel();
            state.pair_requests.lock().await.insert(request_id.clone(), view.clone());
            state.pair_waiters.lock().await.insert(request_id.clone(), sender);
            let _ = state.app.emit("pair-request", view.clone());
            let decision = match timeout(Duration::from_secs(300), receiver).await {
                Ok(Ok(decision)) => decision,
                _ => {
                    state.pair_requests.lock().await.remove(&request_id);
                    state.pair_waiters.lock().await.remove(&request_id);
                    return Err("Eşleşme isteğinin süresi doldu veya iptal edildi.".to_string());
                }
            };

            write_json_line(reader.get_mut(), &PairResponse {
                kind: "pair_response".to_string(),
                accepted: decision.accepted,
                token: decision.token.clone(),
                message: if decision.accepted { None } else { Some("Eşleşme isteği reddedildi.".to_string()) },
            }).await?;

            if decision.accepted {
                state.paired.lock().await.insert(view.device_id.clone(), PairRecord {
                    id: view.device_id,
                    name: view.device_name,
                    token: decision.token.unwrap_or_default(),
                });
                state.save_paired().await?;
                state.emit_devices().await;
            }
            let _ = state.app.emit("pair-queue-updated", ());
        }

        IncomingMessage::TransferRequest { transfer_id, device_id, device_name, token, files } => {
            if transfer_id.len() > 80 || files.is_empty() || files.len() > MAX_FILES_PER_TRANSFER {
                return Err("Aktarım isteği geçersiz.".to_string());
            }
            let authorized = state.paired.lock().await.get(&device_id)
                .map(|record| record.token == token)
                .unwrap_or(false);
            if !authorized {
                write_json_line(reader.get_mut(), &TransferResponse {
                    kind: "transfer_response".to_string(), accepted: false, message: Some("Bu cihaz eşleşmiş değil.".to_string())
                }).await?;
                return Ok(());
            }

            let mut total_bytes = 0u64;
            let mut safe_files = Vec::with_capacity(files.len());
            for file in files {
                let name = sanitize_filename(&file.name);
                total_bytes = total_bytes.checked_add(file.size)
                    .ok_or_else(|| "Dosya boyutları geçersiz.".to_string())?;
                safe_files.push(FileInfo { name, size: file.size });
            }
            if total_bytes > MAX_TRANSFER_BYTES {
                write_json_line(reader.get_mut(), &TransferResponse {
                    kind: "transfer_response".to_string(), accepted: false, message: Some("Aktarım toplam boyut sınırını aşıyor.".to_string())
                }).await?;
                return Ok(());
            }

            let view = TransferRequestView {
                transfer_id: transfer_id.clone(),
                device_id,
                device_name: device_name.chars().take(48).collect(),
                files: safe_files,
                total_bytes,
            };
            let (sender, receiver) = oneshot::channel();
            state.transfer_requests.lock().await.insert(transfer_id.clone(), view.clone());
            state.transfer_waiters.lock().await.insert(transfer_id.clone(), sender);
            let _ = state.app.emit("transfer-request", view.clone());

            let decision = match timeout(Duration::from_secs(600), receiver).await {
                Ok(Ok(decision)) => decision,
                _ => {
                    state.transfer_requests.lock().await.remove(&transfer_id);
                    state.transfer_waiters.lock().await.remove(&transfer_id);
                    return Err("Dosya aktarımı isteğinin süresi doldu veya iptal edildi.".to_string());
                }
            };
            write_json_line(reader.get_mut(), &TransferResponse {
                kind: "transfer_response".to_string(),
                accepted: decision.accepted,
                message: if decision.accepted { None } else { Some("Dosya aktarımı reddedildi.".to_string()) },
            }).await?;
            if !decision.accepted { return Ok(()); }

            let receive_dir = state.data_dir.join("Received");
            let mut received_total = 0u64;
            let mut last_progress_at = std::time::Instant::now();
            if let Some(first_file) = view.files.first() {
                state.emit_transfer_progress(TransferProgress {
                    transfer_id: transfer_id.clone(),
                    direction: "receiving".to_string(),
                    device_name: view.device_name.clone(),
                    file_name: first_file.name.clone(),
                    file_index: 0,
                    file_count: view.files.len(),
                    transferred_bytes: 0,
                    total_bytes: view.total_bytes,
                    status: "active".to_string(),
                });
            }
            fs::create_dir_all(&receive_dir).await.map_err(|error| error.to_string())?;
            for (index, info) in view.files.iter().enumerate() {
                let final_path = unique_target(&receive_dir, &info.name);
                let part_path = receive_dir.join(format!(".fileway-{}-{index}.part", Uuid::new_v4()));
                let mut output = fs::File::create(&part_path)
                    .await
                    .map_err(|error| format!("Geçici dosya oluşturulamadı: {error}"))?;
                let mut remaining = info.size;
                let mut buffer = vec![0u8; IO_CHUNK_BYTES];
                let mut file_error: Option<String> = None;

                while remaining > 0 {
                    let count = remaining.min(buffer.len() as u64) as usize;
                    match timeout(
                        TRANSFER_IDLE_TIMEOUT,
                        reader.read_exact(&mut buffer[..count]),
                    ).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(error)) => {
                            file_error = Some(format!("Dosya aktarımı yarıda kesildi: {error}"));
                            break;
                        }
                        Err(_) => {
                            file_error = Some("Gönderici 90 saniyedir veri göndermiyor; aktarım durduruldu.".to_string());
                            break;
                        }
                    }

                    if let Err(error) = output.write_all(&buffer[..count]).await {
                        file_error = Some(format!("Dosya diske yazılamadı: {error}"));
                        break;
                    }
                    remaining -= count as u64;
                    received_total += count as u64;
                    if last_progress_at.elapsed() >= Duration::from_millis(250)
                        || received_total == view.total_bytes
                    {
                        state.emit_transfer_progress(TransferProgress {
                            transfer_id: transfer_id.clone(),
                            direction: "receiving".to_string(),
                            device_name: view.device_name.clone(),
                            file_name: info.name.clone(),
                            file_index: index,
                            file_count: view.files.len(),
                            transferred_bytes: received_total,
                            total_bytes: view.total_bytes,
                            status: "active".to_string(),
                        });
                        last_progress_at = std::time::Instant::now();
                    }
                }

                if file_error.is_none() {
                    if let Err(error) = output.flush().await {
                        file_error = Some(format!("Dosya diske yazılamadı: {error}"));
                    }
                }
                drop(output);

                if let Some(error) = file_error {
                    let _ = fs::remove_file(&part_path).await;
                    state.emit_transfer_progress(TransferProgress {
                        transfer_id: transfer_id.clone(),
                        direction: "receiving".to_string(),
                        device_name: view.device_name.clone(),
                        file_name: info.name.clone(),
                        file_index: index,
                        file_count: view.files.len(),
                        transferred_bytes: received_total,
                        total_bytes: view.total_bytes,
                        status: "failed".to_string(),
                    });
                    return Err(error);
                }

                if let Err(error) = fs::rename(&part_path, &final_path).await {
                    let _ = fs::remove_file(&part_path).await;
                    state.emit_transfer_progress(TransferProgress {
                        transfer_id: transfer_id.clone(),
                        direction: "receiving".to_string(),
                        device_name: view.device_name.clone(),
                        file_name: info.name.clone(),
                        file_index: index,
                        file_count: view.files.len(),
                        transferred_bytes: received_total,
                        total_bytes: view.total_bytes,
                        status: "failed".to_string(),
                    });
                    return Err(format!("Alınan dosya tamamlandı ancak hedefe taşınamadı: {error}"));
                }
                write_json_line(reader.get_mut(), &FileReceivedMessage {
                    kind: "file_received", file_index: index, ok: true, message: None
                }).await?;
                let received = ReceivedFile {
                    name: info.name.clone(),
                    size: info.size,
                    path: final_path.to_string_lossy().to_string(),
                    modified: unix_millis(),
                };
                let _ = state.app.emit("file-received", received);
            }
            write_json_line(reader.get_mut(), &TransferCompleteMessage { kind: "transfer_complete", ok: true }).await?;
            if let Some(last_file) = view.files.last() {
                state.emit_transfer_progress(TransferProgress {
                    transfer_id: transfer_id.clone(),
                    direction: "receiving".to_string(),
                    device_name: view.device_name.clone(),
                    file_name: last_file.name.clone(),
                    file_index: view.files.len().saturating_sub(1),
                    file_count: view.files.len(),
                    transferred_bytes: view.total_bytes,
                    total_bytes: view.total_bytes,
                    status: "completed".to_string(),
                });
            }
            state.transfer_requests.lock().await.remove(&transfer_id);
            let _ = state.app.emit("transfer-queue-updated", ());
        }
    }
    Ok(())
}

fn unix_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

#[tauri::command]
fn get_device_info(state: State<'_, Arc<AppState>>) -> DeviceInfo {
    state.identity.clone()
}

#[tauri::command]
async fn get_discovered_devices(state: State<'_, Arc<AppState>>) -> Result<Vec<DiscoveredDevice>, String> {
    let now = unix_seconds();
    let paired = state.paired.lock().await;
    let mut devices = state.devices.lock().await;
    devices.retain(|_, item| now.saturating_sub(item.last_seen) <= DEVICE_TTL);
    let mut result = devices.values().cloned().map(|mut item| {
        item.paired = paired.contains_key(&item.id);
        item
    }).collect::<Vec<_>>();
    result.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(result)
}

#[tauri::command]
async fn get_pair_requests(state: State<'_, Arc<AppState>>) -> Result<Vec<PairRequestView>, String> {
    Ok(state.pair_requests.lock().await.values().cloned().collect())
}

#[tauri::command]
async fn get_transfer_requests(state: State<'_, Arc<AppState>>) -> Result<Vec<TransferRequestView>, String> {
    Ok(state.transfer_requests.lock().await.values().cloned().collect())
}

#[tauri::command]
async fn get_paired_devices(state: State<'_, Arc<AppState>>) -> Result<Vec<PairedDeviceView>, String> {
    Ok(state.paired_views().await)
}

#[tauri::command]
async fn scan_now(state: State<'_, Arc<AppState>>) -> Result<(), String> {
    let socket = UdpSocket::bind(("0.0.0.0", 0)).await.map_err(|error| error.to_string())?;
    socket.set_broadcast(true).map_err(|error| error.to_string())?;
    broadcast_beacon(&socket, &state).await;
    Ok(())
}

#[tauri::command]
async fn request_pairing(state: State<'_, Arc<AppState>>, device_id: String) -> Result<(), String> {
    let device = state.devices.lock().await.get(&device_id).cloned()
        .ok_or_else(|| "Cihaz artık ağda görünmüyor. Yeniden tara.".to_string())?;
    if state.paired.lock().await.contains_key(&device_id) {
        return Err("Bu cihaz zaten eşleşmiş.".to_string());
    }
    let address: SocketAddr = format!("{}:{}", device.ip, device.port)
        .parse().map_err(|error| format!("Cihaz adresi geçersiz: {error}"))?;
    let stream = timeout(Duration::from_secs(5), TcpStream::connect(address))
        .await.map_err(|_| "Cihaza bağlanılamadı.".to_string())?
        .map_err(|error| format!("Cihaza bağlanılamadı: {error}"))?;
    let mut reader = BufReader::new(stream);
    let request_id = Uuid::new_v4().to_string();
    write_json_line(reader.get_mut(), &serde_json::json!({
        "kind": "pair_request",
        "requestId": request_id,
        "deviceId": state.identity.id,
        "deviceName": state.identity.name,
    })).await?;

    let line = timeout(Duration::from_secs(300), read_json_line(&mut reader))
        .await.map_err(|_| "Karşı cihaz eşleşme isteğini yanıtlamadı.".to_string())??;
    let response: PairResponse = serde_json::from_str(&line).map_err(|_| "Karşı cihazdan geçersiz yanıt alındı.".to_string())?;
    if !response.accepted {
        return Err(response.message.unwrap_or_else(|| "Eşleşme reddedildi.".to_string()));
    }
    let token = response.token.ok_or_else(|| "Eşleşme anahtarı alınamadı.".to_string())?;
    state.paired.lock().await.insert(device_id.clone(), PairRecord {
        id: device_id,
        name: device.name,
        token,
    });
    state.save_paired().await?;
    state.emit_devices().await;
    Ok(())
}

#[tauri::command]
async fn respond_to_pair_request(
    state: State<'_, Arc<AppState>>,
    request_id: String,
    accept: bool,
) -> Result<(), String> {
    let request = state.pair_requests.lock().await.remove(&request_id)
        .ok_or_else(|| "Eşleşme isteği artık mevcut değil.".to_string())?;
    let sender = state.pair_waiters.lock().await.remove(&request_id)
        .ok_or_else(|| "Eşleşme bağlantısı kapandı.".to_string())?;

    let token = if accept { Some(Uuid::new_v4().to_string()) } else { None };
    if accept {
        state.paired.lock().await.insert(request.device_id.clone(), PairRecord {
            id: request.device_id,
            name: request.device_name,
            token: token.clone().unwrap_or_default(),
        });
        state.save_paired().await?;
    }
    sender.send(PairDecision { accepted: accept, token })
        .map_err(|_| "Karşı cihaz bağlantıyı kapattı.".to_string())?;
    state.emit_devices().await;
    Ok(())
}

#[tauri::command]
async fn remove_paired_device(state: State<'_, Arc<AppState>>, device_id: String) -> Result<(), String> {
    state.paired.lock().await.remove(&device_id);
    state.save_paired().await?;
    state.emit_devices().await;
    Ok(())
}

#[tauri::command]
async fn start_transfer(
    state: State<'_, Arc<AppState>>,
    device_id: String,
    files: Vec<FileInfo>,
) -> Result<String, String> {
    if files.is_empty() || files.len() > MAX_FILES_PER_TRANSFER {
        return Err("En az bir, en fazla 128 dosya seçmelisin.".to_string());
    }
    let paired = state.paired.lock().await.get(&device_id).cloned()
        .ok_or_else(|| "Önce bu cihazla eşleşmelisin.".to_string())?;
    let device = state.devices.lock().await.get(&device_id).cloned()
        .ok_or_else(|| "Alıcı cihaz ağda bulunamadı. Fileway'in açık olduğundan emin ol.".to_string())?;

    let mut total_bytes = 0u64;
    let mut safe_files = Vec::with_capacity(files.len());
    for file in files {
        total_bytes = total_bytes.checked_add(file.size)
            .ok_or_else(|| "Dosya boyutları geçersiz.".to_string())?;
        safe_files.push(FileInfo { name: sanitize_filename(&file.name), size: file.size });
    }
    if total_bytes > MAX_TRANSFER_BYTES {
        return Err("İlk sürümde tek aktarım en fazla 100 GB olabilir.".to_string());
    }

    let address: SocketAddr = format!("{}:{}", device.ip, device.port)
        .parse().map_err(|error| format!("Cihaz adresi geçersiz: {error}"))?;
    let stream = timeout(Duration::from_secs(5), TcpStream::connect(address))
        .await.map_err(|_| "Alıcı cihaza bağlanılamadı.".to_string())?
        .map_err(|error| format!("Alıcı cihaza bağlanılamadı: {error}"))?;
    let mut reader = BufReader::new(stream);
    let transfer_id = Uuid::new_v4().to_string();
    write_json_line(reader.get_mut(), &serde_json::json!({
        "kind": "transfer_request",
        "transferId": transfer_id,
        "deviceId": state.identity.id,
        "deviceName": state.identity.name,
        "token": paired.token,
        "files": safe_files,
    })).await?;

    let line = timeout(Duration::from_secs(600), read_json_line(&mut reader))
        .await.map_err(|_| "Alıcı aktarım isteğini yanıtlamadı.".to_string())??;
    let response: TransferResponse = serde_json::from_str(&line).map_err(|_| "Alıcıdan geçersiz yanıt alındı.".to_string())?;
    if !response.accepted {
        return Err(response.message.unwrap_or_else(|| "Dosya aktarımı reddedildi.".to_string()));
    }
    state.outbound_streams.lock().await.insert(transfer_id.clone(), Arc::new(Mutex::new(reader)));
    Ok(transfer_id)
}

#[tauri::command]
async fn write_file_chunk(
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
    bytes: Vec<u8>,
) -> Result<(), String> {
    if bytes.len() > 1024 * 1024 { return Err("Tek seferde en fazla 1 MiB gönderilebilir.".to_string()); }
    let stream = state.outbound_streams.lock().await.get(&transfer_id).cloned()
        .ok_or_else(|| "Aktarım bağlantısı bulunamadı.".to_string())?;
    let mut reader = stream.lock().await;
    reader.get_mut().write_all(&bytes).await.map_err(|error| error.to_string())
}

#[tauri::command]
async fn finish_file(
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
    file_index: usize,
) -> Result<(), String> {
    let stream = state.outbound_streams.lock().await.get(&transfer_id).cloned()
        .ok_or_else(|| "Aktarım bağlantısı bulunamadı.".to_string())?;
    let mut reader = stream.lock().await;
    let line = timeout(Duration::from_secs(300), read_json_line(&mut reader))
        .await.map_err(|_| "Dosya alıcısının yanıtı zaman aşımına uğradı.".to_string())??;
    let ack: TransferAck = serde_json::from_str(&line).map_err(|_| "Alıcıdan dosya onayı okunamadı.".to_string())?;
    if ack.kind != "file_received" || ack.file_index != Some(file_index) || !ack.ok {
        return Err(ack.message.unwrap_or_else(|| "Alıcı dosyayı kaydedemedi.".to_string()));
    }
    Ok(())
}

#[tauri::command]
async fn finish_transfer(state: State<'_, Arc<AppState>>, transfer_id: String) -> Result<(), String> {
    let stream = state.outbound_streams.lock().await.remove(&transfer_id)
        .ok_or_else(|| "Aktarım bağlantısı bulunamadı.".to_string())?;
    let mut reader = stream.lock().await;
    let line = timeout(Duration::from_secs(300), read_json_line(&mut reader))
        .await.map_err(|_| "Aktarım tamamlanma yanıtı zaman aşımına uğradı.".to_string())??;
    let ack: TransferCompleteAck = serde_json::from_str(&line).map_err(|_| "Aktarım tamamlanma yanıtı okunamadı.".to_string())?;
    if ack.kind != "transfer_complete" || !ack.ok {
        return Err("Alıcı aktarımı tamamlayamadı.".to_string());
    }
    Ok(())
}

#[tauri::command]
async fn cancel_transfer(
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
) -> Result<(), String> {
    state.outbound_streams.lock().await.remove(&transfer_id);
    Ok(())
}

#[tauri::command]
async fn respond_to_transfer(
    state: State<'_, Arc<AppState>>,
    transfer_id: String,
    accept: bool,
) -> Result<(), String> {
    state.transfer_requests.lock().await.remove(&transfer_id)
        .ok_or_else(|| "Dosya aktarımı isteği artık mevcut değil.".to_string())?;
    let sender = state.transfer_waiters.lock().await.remove(&transfer_id)
        .ok_or_else(|| "Gönderen cihaz bağlantıyı kapattı.".to_string())?;
    sender.send(TransferDecision { accepted: accept })
        .map_err(|_| "Gönderen cihaz bağlantıyı kapattı.".to_string())?;
    let _ = state.app.emit("transfer-queue-updated", ());
    Ok(())
}

#[tauri::command]
async fn list_received_files(state: State<'_, Arc<AppState>>) -> Result<Vec<ReceivedFile>, String> {
    let directory = state.data_dir.join("Received");
    if !directory.exists() { return Ok(Vec::new()); }
    let mut entries = fs::read_dir(directory).await.map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|error| error.to_string())? {
        let metadata = entry.metadata().await.map_err(|error| error.to_string())?;
        if !metadata.is_file() || entry.file_name().to_string_lossy().ends_with(".part") { continue; }
        let modified = metadata.modified().ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        files.push(ReceivedFile {
            name: entry.file_name().to_string_lossy().to_string(),
            size: metadata.len(),
            path: entry.path().to_string_lossy().to_string(),
            modified,
        });
    }
    files.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(files)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let state = Arc::new(AppState::new(app.handle())?);
            app.manage(state.clone());
            start_transfer_server(state.clone());
            start_discovery(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_device_info,
            get_discovered_devices,
            get_pair_requests,
            get_transfer_requests,
            get_paired_devices,
            scan_now,
            request_pairing,
            respond_to_pair_request,
            remove_paired_device,
            start_transfer,
            write_file_chunk,
            finish_file,
            finish_transfer,
            cancel_transfer,
            respond_to_transfer,
            list_received_files,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Fileway");
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_sanitization_removes_paths_and_windows_unsafe_characters() {
        assert_eq!(sanitize_filename("../../folder/hello:world?.txt"), "helloworld.txt");
        assert_eq!(sanitize_filename(r"C:\private\secret.txt"), "secret.txt");
        assert_eq!(sanitize_filename("  ..  "), "file");
        assert_eq!(sanitize_filename("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_filename("LPT1.log"), "_LPT1.log");
    }

    #[test]
    fn local_subnet_check_allows_loopback_only_for_local_testing() {
        assert!(is_same_local_subnet(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(is_same_local_subnet(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)));
    }
}
