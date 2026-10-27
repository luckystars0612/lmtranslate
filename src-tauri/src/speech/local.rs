//! The offline voice: Kokoro v1.1-zh (English and Mandarin) run by sherpa-onnx.
//!
//! Nothing of it ships with the app. Settings downloads two pieces on request — the
//! sherpa-onnx runtime libraries and the model — and both are checked against pinned
//! SHA-256 hashes before anything is unpacked. The runtime is loaded with `libloading`
//! only when the voice is first used, so the app itself has no native dependency on it
//! and an install without the offline voice costs nothing.
//!
//! The fp32 model, not the int8 one: measured on a Core Ultra 7, int8 ran at about real
//! time while fp32 was five times faster, which is the difference between the first
//! sentence arriving in a second or in six.

use std::ffi::{c_char, c_void, CString};
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use super::{encode_wav, Clip, SpeechError};
use crate::config;
use crate::state::AppState;

pub const STATUS_EVENT: &str = "speech:offline";

const SHERPA_VERSION: &str = "v1.12.9";
const MODEL_NAME: &str = "kokoro-multi-lang-v1_1";
const MODEL_URL: &str =
    "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_1.tar.bz2";
const MODEL_SHA256: &str = "b78e1ac42b852e839cceaafbe88f66806bf5860992d1d7b9a3f2ad1198427aed";
const MODEL_BYTES: u64 = 364_855_331;

/// Written last, so a half-finished download or unpack never counts as installed.
const COMPLETE_MARKER: &str = ".complete";
/// The model is several hundred MB in memory; give it back when the voice is not in use.
const IDLE_UNLOAD: Duration = Duration::from_secs(10 * 60);
/// More threads stopped helping past eight in measurement.
const MAX_THREADS: usize = 8;

/// The prebuilt sherpa-onnx shared libraries for this platform. The hashes are the ones
/// sherpa-rs publishes for the same release.
struct Runtime {
    archive: &'static str,
    sha256: &'static str,
    bytes: u64,
    api_library: &'static str,
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const RUNTIME: Option<Runtime> = Some(Runtime {
    archive: "sherpa-onnx-v1.12.9-win-x64-shared.tar.bz2",
    sha256: "57a0dff63c468ed067c6ffd1a9c9b6f66f4cf73304d44b5f17518010ce2e0977",
    bytes: 24_139_676,
    api_library: "sherpa-onnx-c-api.dll",
});

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
const RUNTIME: Option<Runtime> = Some(Runtime {
    archive: "sherpa-onnx-v1.12.9-linux-x64-shared.tar.bz2",
    sha256: "cd22ee337e205674536643d2bc86e984fbbbf9c5c52743c4df6fc8d8dd17371b",
    // Not measured; only the model's size is shown on this platform.
    bytes: 0,
    api_library: "libsherpa-onnx-c-api.so",
});

#[cfg(target_os = "macos")]
const RUNTIME: Option<Runtime> = Some(Runtime {
    archive: "sherpa-onnx-v1.12.9-osx-universal2-shared.tar.bz2",
    sha256: "e3f3596b84c65eef8939a8a45ced3d7c4ed857455ba5ae1148ce7a1c93ff9a3c",
    // Not measured; only the model's size is shown on this platform.
    bytes: 0,
    api_library: "libsherpa-onnx-c-api.dylib",
});

#[cfg(not(any(
    all(target_os = "windows", target_arch = "x86_64"),
    all(target_os = "linux", target_arch = "x86_64"),
    target_os = "macos"
)))]
const RUNTIME: Option<Runtime> = None;

/// What Settings shows about the offline voice.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OfflineStatus {
    pub supported: bool,
    pub installed: bool,
    pub installing: bool,
    /// "runtime" | "model" | "unpacking" while installing.
    pub stage: String,
    pub received: u64,
    pub total: u64,
    /// Rough total download, for the button label.
    pub download_mb: u64,
    pub error: String,
}

/// The two ways the model is loaded. Chinese needs sherpa-onnx's number and date rules,
/// and those same rules read the digits in an English sentence in Chinese — so English
/// is loaded without them. Only one is kept in memory at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Profile {
    English,
    Chinese,
}

#[derive(Default)]
pub struct LocalState {
    loaded: Mutex<Option<Loaded>>,
    status: Mutex<OfflineStatus>,
}

struct Loaded {
    profile: Profile,
    voice: LocalVoice,
    last_used: Instant,
}

// ── Status ─────────────────────────────────────────────────────────────────

pub fn status(app: &AppHandle) -> OfflineStatus {
    let mut status = app
        .try_state::<AppState>()
        .and_then(|state| state.speech.local.status.lock().ok().map(|s| s.clone()))
        .unwrap_or_default();
    status.supported = RUNTIME.is_some();
    status.installed = is_installed(app);
    status.download_mb = (RUNTIME.as_ref().map_or(0, |r| r.bytes) + MODEL_BYTES) / 1_000_000;
    status
}

pub fn is_installed(app: &AppHandle) -> bool {
    let Some(runtime) = RUNTIME.as_ref() else {
        return false;
    };
    let Ok(dir) = config::voices_dir(app) else {
        return false;
    };
    let runtime_dir = dir.join("runtime");
    runtime_dir.join(COMPLETE_MARKER).exists()
        && runtime_dir.join(runtime.api_library).exists()
        && dir.join(MODEL_NAME).join(COMPLETE_MARKER).exists()
}

fn update_status(app: &AppHandle, change: impl FnOnce(&mut OfflineStatus)) {
    if let Some(state) = app.try_state::<AppState>() {
        if let Ok(mut status) = state.speech.local.status.lock() {
            change(&mut status);
        }
    }
    let _ = app.emit(STATUS_EVENT, status(app));
}

// ── Install / remove ───────────────────────────────────────────────────────

/// Downloads and unpacks the runtime and the model. Safe to call again after a failure:
/// whatever already finished is kept.
pub async fn install(app: AppHandle) -> Result<(), String> {
    let Some(runtime) = RUNTIME.as_ref() else {
        return Err("The offline voice is not available for this platform.".into());
    };

    {
        let state = app.state::<AppState>();
        let mut status = state.speech.local.status.lock().map_err(|e| e.to_string())?;
        if status.installing {
            return Err("The offline voice is already downloading.".into());
        }
        *status = OfflineStatus {
            installing: true,
            ..Default::default()
        };
    }
    update_status(&app, |_| {});

    let outcome = install_steps(&app, runtime).await;

    update_status(&app, |status| {
        status.installing = false;
        status.stage.clear();
        status.error = outcome.as_ref().err().cloned().unwrap_or_default();
    });
    match &outcome {
        Ok(()) => log(&app, "offline voice installed"),
        Err(e) => log(&app, &format!("offline voice install failed: {e}")),
    }
    outcome
}

async fn install_steps(app: &AppHandle, runtime: &Runtime) -> Result<(), String> {
    let dir = config::voices_dir(app)?;
    let downloads = dir.join("downloads");
    fs::create_dir_all(&downloads).map_err(|e| format!("could not create {downloads:?}: {e}"))?;

    let runtime_dir = dir.join("runtime");
    if !runtime_dir.join(COMPLETE_MARKER).exists() {
        let archive = downloads.join(runtime.archive);
        let url = format!(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/{SHERPA_VERSION}/{}",
            runtime.archive
        );
        download(app, &url, &archive, runtime.sha256, "runtime").await?;
        update_status(app, |s| s.stage = "unpacking".into());
        let target = runtime_dir.clone();
        tauri::async_runtime::spawn_blocking(move || unpack_runtime(&archive, &target))
            .await
            .map_err(|e| e.to_string())??;
        mark_complete(&runtime_dir)?;
    }

    let model_dir = dir.join(MODEL_NAME);
    if !model_dir.join(COMPLETE_MARKER).exists() {
        let archive = downloads.join(format!("{MODEL_NAME}.tar.bz2"));
        download(app, MODEL_URL, &archive, MODEL_SHA256, "model").await?;
        update_status(app, |s| s.stage = "unpacking".into());
        let target = dir.clone();
        tauri::async_runtime::spawn_blocking(move || unpack_all(&archive, &target))
            .await
            .map_err(|e| e.to_string())??;
        mark_complete(&model_dir)?;
    }

    let _ = fs::remove_dir_all(&downloads);
    Ok(())
}

/// Deletes everything the install put on disk.
pub fn remove(app: &AppHandle) -> Result<(), String> {
    if let Some(state) = app.try_state::<AppState>() {
        if state.speech.local.status.lock().map(|s| s.installing).unwrap_or(false) {
            return Err("Wait for the download to finish first.".into());
        }
        if let Ok(mut loaded) = state.speech.local.loaded.lock() {
            *loaded = None;
        }
    }
    let dir = config::voices_dir(app)?;
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("could not delete {dir:?}: {e}"))?;
    }
    update_status(app, |status| status.error.clear());
    log(app, "offline voice removed");
    Ok(())
}

async fn download(
    app: &AppHandle,
    url: &str,
    dest: &Path,
    sha256: &str,
    stage: &str,
) -> Result<(), String> {
    // Separate from the cloud voices' client: that one gives a whole request 30 seconds,
    // and a 365 MB download can take minutes.
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .user_agent(concat!("lmtranslate/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())?;

    let mut response = http
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("could not download the offline voice: {e}"))?;

    let total = response.content_length().unwrap_or(0);
    update_status(app, |s| {
        s.stage = stage.into();
        s.received = 0;
        s.total = total;
    });

    let partial = dest.with_extension("part");
    let mut file = File::create(&partial).map_err(|e| format!("could not write {partial:?}: {e}"))?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut last_report = Instant::now();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("the download was interrupted: {e}"))?
    {
        file.write_all(&chunk).map_err(|e| format!("could not write {partial:?}: {e}"))?;
        hasher.update(&chunk);
        received += chunk.len() as u64;
        if last_report.elapsed() > Duration::from_millis(250) {
            last_report = Instant::now();
            update_status(app, |s| s.received = received);
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);

    let actual = format!("{:x}", hasher.finalize());
    if actual != sha256 {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "the downloaded {stage} did not match its published checksum, so it was discarded"
        ));
    }
    fs::rename(&partial, dest).map_err(|e| format!("could not move {partial:?}: {e}"))?;
    Ok(())
}

/// Only the shared libraries; the archive also carries headers and command-line tools.
fn unpack_runtime(archive: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    let mut tar = open_archive(archive)?;
    for entry in tar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let in_lib = path.components().any(|c| c.as_os_str() == "lib");
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
            continue;
        };
        if in_lib && is_shared_library(&name) {
            entry
                .unpack(target.join(&name))
                .map_err(|e| format!("could not unpack {name}: {e}"))?;
        }
    }
    Ok(())
}

fn unpack_all(archive: &Path, target: &Path) -> Result<(), String> {
    // `unpack` refuses entries that would land outside `target`.
    open_archive(archive)?
        .unpack(target)
        .map_err(|e| format!("could not unpack the model: {e}"))
}

fn open_archive(path: &Path) -> Result<tar::Archive<bzip2::read::BzDecoder<File>>, String> {
    let file = File::open(path).map_err(|e| format!("could not open {path:?}: {e}"))?;
    Ok(tar::Archive::new(bzip2::read::BzDecoder::new(file)))
}

fn mark_complete(dir: &Path) -> Result<(), String> {
    fs::write(dir.join(COMPLETE_MARKER), b"").map_err(|e| e.to_string())
}

fn is_shared_library(name: &str) -> bool {
    name.ends_with(".dll") || name.ends_with(".dylib") || name.contains(".so")
}

// ── Synthesis ──────────────────────────────────────────────────────────────

/// Blocking: loads the voice on first use (a few seconds) and keeps it for the next.
pub fn synthesize(app: &AppHandle, text: &str, speaker: i32) -> Result<Clip, SpeechError> {
    let state = app.state::<AppState>();
    let profile = if super::voices::is_chinese_speaker(speaker) {
        Profile::Chinese
    } else {
        Profile::English
    };

    let mut slot = state
        .speech
        .local
        .loaded
        .lock()
        .map_err(|_| SpeechError::Failed("the offline voice is unavailable".into()))?;

    if slot.as_ref().map(|l| l.profile) != Some(profile) {
        // Drop the other profile before loading this one: both at once would double the
        // memory for no benefit.
        *slot = None;
        let started = Instant::now();
        let dir = config::voices_dir(app).map_err(SpeechError::Failed)?;
        let voice = LocalVoice::load(&dir, profile)?;
        log(
            app,
            &format!(
                "offline voice loaded ({profile:?}) in {}ms",
                started.elapsed().as_millis()
            ),
        );
        *slot = Some(Loaded {
            profile,
            voice,
            last_used: Instant::now(),
        });
    }

    let loaded = slot.as_mut().expect("loaded just above");
    let (samples, sample_rate) = loaded.voice.generate(text, speaker)?;
    loaded.last_used = Instant::now();
    drop(slot);

    schedule_unload(app);
    Ok(Clip {
        mime: "audio/wav",
        bytes: encode_wav(&samples, sample_rate),
    })
}

fn schedule_unload(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(IDLE_UNLOAD).await;
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let Ok(mut slot) = state.speech.local.loaded.lock() else {
            return;
        };
        if slot.as_ref().is_some_and(|l| l.last_used.elapsed() >= IDLE_UNLOAD) {
            *slot = None;
            state.logger.info("offline voice unloaded after being idle");
        }
    });
}

// ── sherpa-onnx C API ──────────────────────────────────────────────────────
//
// Mirrors c-api.h of sherpa-onnx v1.12.9 — the version pinned above. The layout must
// match that header exactly; update both together.

#[repr(C)]
struct VitsConfig {
    model: *const c_char,
    lexicon: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    noise_scale: f32,
    noise_scale_w: f32,
    length_scale: f32,
    dict_dir: *const c_char,
}

#[repr(C)]
struct MatchaConfig {
    acoustic_model: *const c_char,
    vocoder: *const c_char,
    lexicon: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    noise_scale: f32,
    length_scale: f32,
    dict_dir: *const c_char,
}

#[repr(C)]
struct KokoroConfig {
    model: *const c_char,
    voices: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    length_scale: f32,
    dict_dir: *const c_char,
    lexicon: *const c_char,
    lang: *const c_char,
}

#[repr(C)]
struct KittenConfig {
    model: *const c_char,
    voices: *const c_char,
    tokens: *const c_char,
    data_dir: *const c_char,
    length_scale: f32,
}

#[repr(C)]
struct ModelConfig {
    vits: VitsConfig,
    num_threads: i32,
    debug: i32,
    provider: *const c_char,
    matcha: MatchaConfig,
    kokoro: KokoroConfig,
    kitten: KittenConfig,
}

#[repr(C)]
struct TtsConfig {
    model: ModelConfig,
    rule_fsts: *const c_char,
    max_num_sentences: i32,
    rule_fars: *const c_char,
    silence_scale: f32,
}

#[repr(C)]
struct GeneratedAudio {
    samples: *const f32,
    n: i32,
    sample_rate: i32,
}

type CreateFn = unsafe extern "C" fn(*const TtsConfig) -> *const c_void;
type DestroyFn = unsafe extern "C" fn(*const c_void);
type GenerateFn = unsafe extern "C" fn(*const c_void, *const c_char, i32, f32) -> *const GeneratedAudio;
type DestroyAudioFn = unsafe extern "C" fn(*const GeneratedAudio);

struct LocalVoice {
    tts: *const c_void,
    generate: GenerateFn,
    destroy: DestroyFn,
    destroy_audio: DestroyAudioFn,
    /// Kept alive for as long as the function pointers above are used.
    _libraries: Vec<libloading::Library>,
}

// The handle is only ever used under `LocalState::loaded`'s mutex.
unsafe impl Send for LocalVoice {}

impl LocalVoice {
    fn load(voices_dir: &Path, profile: Profile) -> Result<Self, SpeechError> {
        let runtime = RUNTIME
            .as_ref()
            .ok_or_else(|| SpeechError::NotConfigured("no offline runtime here".into()))?;
        let runtime_dir = voices_dir.join("runtime");
        let model_dir = voices_dir.join(MODEL_NAME);

        let failed = |what: &str, e: libloading::Error| {
            SpeechError::Failed(format!("could not load the offline voice ({what}): {e}"))
        };

        // ONNX Runtime first, so the API library's dependency on it resolves to this
        // copy rather than to whatever else might be on the search path.
        let mut libraries = Vec::new();
        let entries = fs::read_dir(&runtime_dir)
            .map_err(|e| SpeechError::NotConfigured(format!("offline voice not installed: {e}")))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.contains("onnxruntime") && !name.contains("providers") && is_shared_library(&name)
            {
                libraries.push(open_library(&entry.path()).map_err(|e| failed(&name, e))?);
            }
        }
        let api = open_library(&runtime_dir.join(runtime.api_library))
            .map_err(|e| failed(runtime.api_library, e))?;

        let (create, generate, destroy, destroy_audio) = unsafe {
            (
                *api.get::<CreateFn>(b"SherpaOnnxCreateOfflineTts\0")
                    .map_err(|e| failed("create", e))?,
                *api.get::<GenerateFn>(b"SherpaOnnxOfflineTtsGenerate\0")
                    .map_err(|e| failed("generate", e))?,
                *api.get::<DestroyFn>(b"SherpaOnnxDestroyOfflineTts\0")
                    .map_err(|e| failed("destroy", e))?,
                *api.get::<DestroyAudioFn>(b"SherpaOnnxDestroyOfflineTtsGeneratedAudio\0")
                    .map_err(|e| failed("destroy audio", e))?,
            )
        };
        libraries.push(api);

        let path = |name: &str| c_path(&model_dir.join(name));
        let model = path("model.onnx");
        let voices = path("voices.bin");
        let tokens = path("tokens.txt");
        let data_dir = path("espeak-ng-data");
        let dict_dir = path("dict");
        let lexicon = c_string(&format!(
            "{},{}",
            path_string(&model_dir.join("lexicon-us-en.txt")),
            path_string(&model_dir.join("lexicon-zh.txt"))
        ));
        let rule_fsts = match profile {
            Profile::Chinese => c_string(
                &["phone-zh.fst", "date-zh.fst", "number-zh.fst"]
                    .map(|f| path_string(&model_dir.join(f)))
                    .join(","),
            ),
            Profile::English => c_string(""),
        };
        let provider = c_string("cpu");
        let empty = c_string("");

        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(MAX_THREADS))
            .unwrap_or(4) as i32;

        // Every string is copied by sherpa-onnx during creation, so these CStrings only
        // need to outlive the call.
        let tts = unsafe {
            let mut config: TtsConfig = std::mem::zeroed();
            config.model.num_threads = threads;
            config.model.provider = provider.as_ptr();
            config.model.kokoro = KokoroConfig {
                model: model.as_ptr(),
                voices: voices.as_ptr(),
                tokens: tokens.as_ptr(),
                data_dir: data_dir.as_ptr(),
                length_scale: 1.0,
                dict_dir: dict_dir.as_ptr(),
                lexicon: lexicon.as_ptr(),
                lang: empty.as_ptr(),
            };
            config.rule_fsts = rule_fsts.as_ptr();
            config.rule_fars = empty.as_ptr();
            config.max_num_sentences = 1;
            config.silence_scale = 0.2;
            create(&config)
        };
        if tts.is_null() {
            return Err(SpeechError::Failed(
                "the offline voice files are damaged; remove and download it again".into(),
            ));
        }

        Ok(Self {
            tts,
            generate,
            destroy,
            destroy_audio,
            _libraries: libraries,
        })
    }

    fn generate(&self, text: &str, speaker: i32) -> Result<(Vec<f32>, u32), SpeechError> {
        let text = c_string(text);
        unsafe {
            let audio = (self.generate)(self.tts, text.as_ptr(), speaker, 1.0);
            if audio.is_null() {
                return Err(SpeechError::Failed("the offline voice produced nothing".into()));
            }
            let result = if (*audio).samples.is_null() || (*audio).n <= 0 {
                Err(SpeechError::Failed("the offline voice produced nothing".into()))
            } else {
                Ok((
                    std::slice::from_raw_parts((*audio).samples, (*audio).n as usize).to_vec(),
                    (*audio).sample_rate as u32,
                ))
            };
            (self.destroy_audio)(audio);
            result
        }
    }
}

impl Drop for LocalVoice {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.tts) };
    }
}

#[cfg(windows)]
fn open_library(path: &Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::windows::{Library, LOAD_WITH_ALTERED_SEARCH_PATH};
    // Resolve the library's own dependencies from its folder, not the app's.
    unsafe { Library::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH).map(Into::into) }
}

#[cfg(unix)]
fn open_library(path: &Path) -> Result<libloading::Library, libloading::Error> {
    use libloading::os::unix::{Library, RTLD_GLOBAL, RTLD_NOW};
    unsafe { Library::open(Some(path), RTLD_NOW | RTLD_GLOBAL).map(Into::into) }
}

fn c_string(value: &str) -> CString {
    CString::new(value.replace('\0', " ")).unwrap_or_default()
}

fn c_path(path: &Path) -> CString {
    c_string(&path_string(path))
}

/// sherpa-onnx opens files through narrow-character APIs, which on Windows cannot reach
/// a path with characters outside the system code page — a Vietnamese user name in the
/// AppData path, for instance. The 8.3 short form of the same path is plain ASCII.
#[cfg(windows)]
fn path_string(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;

    let long = path.to_string_lossy().into_owned();
    if long.is_ascii() {
        return long;
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buffer = vec![0u16; 1024];
    let len = unsafe { GetShortPathNameW(wide.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
    if len == 0 || len as usize >= buffer.len() {
        return long;
    }
    String::from_utf16_lossy(&buffer[..len as usize])
}

#[cfg(not(windows))]
fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn log(app: &AppHandle, message: &str) {
    if let Some(state) = app.try_state::<AppState>() {
        state.logger.info(message);
    }
}
