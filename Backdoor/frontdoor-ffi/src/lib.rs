//! C ABI for the native Pluto frontend.
//!
//! The C application loads this crate as a shared library at runtime (see
//! `New Frontdoor/pluto_bridge.c`).  All strings are UTF-8 and all returned
//! buffers are owned by Rust until `pluto_buffer_free` is called.
//!
//! Conventions:
//! * functions returning `i32` use `1` for success and `0` for failure;
//! * `pluto_last_error()` returns the message for the most recent failure on
//!   the calling thread;
//! * `PlutoClipInfo` is layout-compatible with the C struct of the same name.

use pluto_media_engine::{
    ffmpeg_filmstrip, ffmpeg_preview_frame, ffmpeg_thumbnail, ffmpeg_waveform, new_export_process,
    probe_media, render_timeline, ExportProcess,
};
use pluto_timeline_core::{Clip, Timeline, TrackId};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

const ID_SIZE: usize = 64;
const TRACK_SIZE: usize = 16;
const LABEL_SIZE: usize = 128;
const PATH_SIZE: usize = 1024;
const KIND_SIZE: usize = 16;
const THEME_SIZE: usize = 24;
const TRANS_SIZE: usize = 16;
const MESSAGE_SIZE: usize = 512;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PlutoClipInfo {
    pub id: [c_char; ID_SIZE],
    pub track: [c_char; TRACK_SIZE],
    pub start: f64,
    pub dur: f64,
    pub label: [c_char; LABEL_SIZE],
    pub linked_audio_id: [c_char; ID_SIZE],
    pub path: [c_char; PATH_SIZE],
    pub media_kind: [c_char; KIND_SIZE],
    pub theme: [c_char; THEME_SIZE],
    pub has_audio: i32,
    pub source_offset: f64,
    pub source_duration: f64,
    pub trans_type: [c_char; TRANS_SIZE],
    pub trans_dur: f64,
}

impl Default for PlutoClipInfo {
    fn default() -> Self {
        Self {
            id: [0; ID_SIZE],
            track: [0; TRACK_SIZE],
            start: 0.0,
            dur: 0.0,
            label: [0; LABEL_SIZE],
            linked_audio_id: [0; ID_SIZE],
            path: [0; PATH_SIZE],
            media_kind: [0; KIND_SIZE],
            theme: [0; THEME_SIZE],
            has_audio: 0,
            source_offset: 0.0,
            source_duration: 0.0,
            trans_type: [0; TRANS_SIZE],
            trans_dur: 0.0,
        }
    }
}

#[repr(C)]
pub struct PlutoMediaInfo {
    pub name: [c_char; 256],
    pub path: [c_char; PATH_SIZE],
    pub kind: [c_char; KIND_SIZE],
    pub duration: f64,
    pub has_audio: i32,
}

#[repr(C)]
pub struct PlutoExportStatus {
    pub running: i32,
    pub finished: i32,
    pub success: i32,
    pub canceled: i32,
    pub progress: f64,
    pub message: [c_char; MESSAGE_SIZE],
    pub output: [c_char; PATH_SIZE],
}

#[derive(Clone)]
struct ClipMeta {
    path: String,
    media_kind: String,
    theme: String,
    has_audio: bool,
    source_offset: f64,
    source_duration: f64,
    trans_type: String,
    trans_dur: f64,
    props: Value,
}

struct Backend {
    timeline: Timeline,
    meta: HashMap<String, ClipMeta>,
}

fn backend() -> &'static Mutex<Backend> {
    static BACKEND: OnceLock<Mutex<Backend>> = OnceLock::new();
    BACKEND.get_or_init(|| {
        Mutex::new(Backend {
            timeline: Timeline::default(),
            meta: HashMap::new(),
        })
    })
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::new("").expect("empty CString"));
}

fn set_error(message: impl Into<String>) {
    let message = message.into();
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::new(message.replace('\0', " ")).unwrap_or_default();
    });
}

fn clear_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::new("").expect("empty CString");
    });
}

fn write_cstr(dst: &mut [c_char], value: &str) {
    let len = dst.len();
    let bytes = value.as_bytes();
    let count = bytes.len().min(len.saturating_sub(1));
    for (index, byte) in bytes.iter().take(count).enumerate() {
        dst[index] = *byte as c_char;
    }
    for slot in dst.iter_mut().skip(count) {
        *slot = 0;
    }
}

unsafe fn read_cstr(value: &[c_char]) -> String {
    CStr::from_ptr(value.as_ptr()).to_string_lossy().trim().to_owned()
}

unsafe fn read_arg(value: *const c_char) -> Result<String, String> {
    if value.is_null() {
        return Err("A required string argument was null".into());
    }
    CStr::from_ptr(value)
        .to_str()
        .map(|text| text.to_owned())
        .map_err(|_| "A string argument was not valid UTF-8".to_owned())
}

fn track_from_str(value: &str) -> Result<TrackId, String> {
    match value {
        "v1" => Ok(TrackId::V1),
        "v2" => Ok(TrackId::V2),
        "v3" => Ok(TrackId::V3),
        "v4" => Ok(TrackId::V4),
        "a1" => Ok(TrackId::A1),
        "a2" => Ok(TrackId::A2),
        other => Err(format!("Unknown track '{other}'")),
    }
}

fn track_name(track: TrackId) -> &'static str {
    match track {
        TrackId::V1 => "v1",
        TrackId::V2 => "v2",
        TrackId::V3 => "v3",
        TrackId::V4 => "v4",
        TrackId::A1 => "a1",
        TrackId::A2 => "a2",
    }
}

fn parse_props(value: &str) -> Result<Value, String> {
    if value.trim().is_empty() {
        return Ok(json!({}));
    }
    let parsed: Value = serde_json::from_str(value).map_err(|error| format!("Timeline properties are not valid JSON: {error}"))?;
    if !parsed.is_object() {
        return Err("Timeline properties must be a JSON object".into());
    }
    Ok(parsed)
}

unsafe fn clip_from_info(info: &PlutoClipInfo) -> Result<(Clip, ClipMeta), String> {
    let id = read_cstr(&info.id);
    let track = track_from_str(&read_cstr(&info.track))?;
    let label = read_cstr(&info.label);
    let linked = read_cstr(&info.linked_audio_id);
    if id.is_empty() {
        return Err("Clip id is required".into());
    }
    let clip = Clip {
        id: id.clone(),
        track,
        start: info.start,
        duration: info.dur,
        label: if label.is_empty() { id.clone() } else { label },
        linked_clip_id: (!linked.is_empty()).then_some(linked),
        extra: HashMap::new(),
    };
    let meta = ClipMeta {
        path: read_cstr(&info.path),
        media_kind: read_cstr(&info.media_kind),
        theme: read_cstr(&info.theme),
        has_audio: info.has_audio != 0,
        source_offset: info.source_offset,
        source_duration: info.source_duration,
        trans_type: read_cstr(&info.trans_type),
        trans_dur: info.trans_dur,
        props: json!({}),
    };
    Ok((clip, meta))
}

fn fill_info(clip: &Clip, meta: Option<&ClipMeta>, out: &mut PlutoClipInfo) {
    write_cstr(&mut out.id, &clip.id);
    write_cstr(&mut out.track, track_name(clip.track));
    out.start = clip.start;
    out.dur = clip.duration;
    write_cstr(&mut out.label, &clip.label);
    write_cstr(&mut out.linked_audio_id, clip.linked_clip_id.as_deref().unwrap_or(""));
    if let Some(meta) = meta {
        write_cstr(&mut out.path, &meta.path);
        write_cstr(&mut out.media_kind, &meta.media_kind);
        write_cstr(&mut out.theme, &meta.theme);
        out.has_audio = i32::from(meta.has_audio);
        out.source_offset = meta.source_offset;
        out.source_duration = meta.source_duration;
        write_cstr(&mut out.trans_type, &meta.trans_type);
        out.trans_dur = meta.trans_dur;
    } else {
        out.has_audio = 0;
        out.source_offset = 0.0;
        out.source_duration = 0.0;
        write_cstr(&mut out.trans_type, "");
        out.trans_dur = 0.0;
    }
}

fn export_clip(clip: &Clip, meta: &ClipMeta) -> Value {
    let trans = if meta.trans_type.is_empty() {
        Value::Null
    } else {
        json!({ "type": meta.trans_type, "dur": meta.trans_dur })
    };
    json!({
        "id": clip.id,
        "track": track_name(clip.track),
        "start": clip.start,
        "dur": clip.duration,
        "label": clip.label,
        "linkedAudioId": clip.linked_clip_id,
        "path": meta.path,
        "mediaKind": meta.media_kind,
        "theme": meta.theme,
        "hasAudio": meta.has_audio,
        "sourceOffset": meta.source_offset,
        "sourceDuration": meta.source_duration,
        "trans": trans,
        "props": meta.props,
    })
}

fn c_buffer(bytes: Vec<u8>) -> (*mut u8, i32) {
    let length = bytes.len();
    let mut boxed = bytes.into_boxed_slice();
    let pointer = boxed.as_mut_ptr();
    std::mem::forget(boxed);
    (pointer, length as i32)
}

// -----------------------------------------------------------------------------
// Lifecycle and diagnostics
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn pluto_backend_init() -> i32 {
    match backend().lock() {
        Ok(mut state) => {
            let snapping = state.timeline.snapping;
            state.timeline = Timeline::default();
            state.timeline.snapping = snapping;
            state.meta.clear();
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn pluto_backend_shutdown() {
    if let Ok(mut state) = backend().lock() {
        state.timeline = Timeline::default();
        state.meta.clear();
    }
    pluto_export_cancel();
}

#[no_mangle]
pub extern "C" fn pluto_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}

// -----------------------------------------------------------------------------
// Timeline
// -----------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn pluto_timeline_reset() -> i32 {
    match backend().lock() {
        Ok(mut state) => {
            let snapping = state.timeline.snapping;
            state.timeline = Timeline::default();
            state.timeline.snapping = snapping;
            state.meta.clear();
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn pluto_timeline_count() -> i32 {
    backend().lock().map(|state| state.timeline.clips.len() as i32).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn pluto_timeline_duration() -> f64 {
    backend().lock().map(|state| state.timeline.duration_seconds).unwrap_or(0.0)
}

#[no_mangle]
pub extern "C" fn pluto_timeline_snapping() -> i32 {
    backend()
        .lock()
        .map(|state| i32::from(state.timeline.snapping))
        .unwrap_or(1)
}

#[no_mangle]
pub unsafe extern "C" fn pluto_timeline_get_clip(index: i32, out: *mut PlutoClipInfo) -> i32 {
    if out.is_null() {
        set_error("Clip output pointer was null");
        return 0;
    }
    match backend().lock() {
        Ok(state) => {
            let Some(clip) = usize::try_from(index).ok().and_then(|index| state.timeline.clips.get(index)) else {
                set_error("Clip index is out of range");
                return 0;
            };
            let mut info = PlutoClipInfo::default();
            fill_info(clip, state.meta.get(&clip.id), &mut info);
            *out = info;
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pluto_timeline_add_clip(info: *const PlutoClipInfo, props_json: *const c_char) -> i32 {
    if info.is_null() {
        set_error("Clip info pointer was null");
        return 0;
    }
    let props = match read_arg(props_json) {
        Ok(text) => match parse_props(&text) {
            Ok(value) => value,
            Err(error) => {
                set_error(error);
                return 0;
            }
        },
        Err(_) => json!({}),
    };
    let (clip, mut meta) = match clip_from_info(&*info) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    meta.props = props;
    match backend().lock() {
        Ok(mut state) => {
            if let Err(error) = state.timeline.add_clip(clip.clone()) {
                set_error(format!("{error:?}"));
                return 0;
            }
            state.meta.insert(clip.id.clone(), meta);
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pluto_timeline_set_props(id: *const c_char, props_json: *const c_char) -> i32 {
    let id = match read_arg(id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let props = match read_arg(props_json) {
        Ok(text) => match parse_props(&text) {
            Ok(value) => value,
            Err(error) => {
                set_error(error);
                return 0;
            }
        },
        Err(_) => json!({}),
    };
    match backend().lock() {
        Ok(mut state) => {
            if !state.timeline.clips.iter().any(|clip| clip.id == id) {
                set_error(format!("Clip '{id}' was not found"));
                return 0;
            }
            match state.meta.get_mut(&id) {
                Some(meta) => meta.props = props,
                None => {
                    set_error(format!("Clip '{id}' has no metadata"));
                    return 0;
                }
            }
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pluto_timeline_set_transition(id: *const c_char, kind: *const c_char, dur: f64) -> i32 {
    let id = match read_arg(id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let kind = read_arg(kind).unwrap_or_default();
    match backend().lock() {
        Ok(mut state) => {
            if !state.timeline.clips.iter().any(|clip| clip.id == id) {
                set_error(format!("Clip '{id}' was not found"));
                return 0;
            }
            match state.meta.get_mut(&id) {
                Some(meta) => {
                    if kind.is_empty() || kind == "none" {
                        meta.trans_type.clear();
                        meta.trans_dur = 0.0;
                    } else {
                        meta.trans_type = kind;
                        meta.trans_dur = if dur.is_finite() { dur.clamp(0.05, 10.0) } else { 1.0 };
                    }
                }
                None => {
                    set_error(format!("Clip '{id}' has no metadata"));
                    return 0;
                }
            }
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pluto_timeline_move_clip(
    id: *const c_char,
    start: f64,
    track: *const c_char,
    playhead: f64,
    snap_threshold: f64,
) -> i32 {
    let id = match read_arg(id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let track = match read_arg(track).and_then(|value| track_from_str(&value)) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    match backend().lock() {
        Ok(mut state) => {
            if let Err(error) = state.timeline.move_clip(&id, start, track, playhead, snap_threshold) {
                set_error(format!("{error:?}"));
                return 0;
            }
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pluto_timeline_trim_clip(id: *const c_char, start: f64, end: f64) -> i32 {
    let id = match read_arg(id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    match backend().lock() {
        Ok(mut state) => {
            if let Err(error) = state.timeline.trim_clip(&id, start, end) {
                set_error(format!("{error:?}"));
                return 0;
            }
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pluto_timeline_split_clip(id: *const c_char, at: f64, right_id: *const c_char) -> i32 {
    let id = match read_arg(id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let right_id = match read_arg(right_id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    match backend().lock() {
        Ok(mut state) => {
            if let Err(error) = state.timeline.split_clip(&id, at, right_id.clone()) {
                set_error(format!("{error:?}"));
                return 0;
            }
            if let Some(meta) = state.meta.get(&id).cloned() {
                let mut right_meta = meta;
                right_meta.trans_type.clear();
                right_meta.trans_dur = 0.0;
                state.meta.insert(right_id, right_meta);
            }
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pluto_timeline_remove_clip(id: *const c_char) -> i32 {
    let id = match read_arg(id) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    match backend().lock() {
        Ok(mut state) => match state.timeline.delete_clip(&id) {
            Ok(deleted) => {
                for clip in deleted {
                    state.meta.remove(&clip.id);
                }
                clear_error();
                1
            }
            Err(error) => {
                set_error(format!("{error:?}"));
                0
            }
        },
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn pluto_timeline_set_snapping(enabled: i32) -> i32 {
    match backend().lock() {
        Ok(mut state) => {
            state.timeline.snapping = enabled != 0;
            clear_error();
            1
        }
        Err(_) => {
            set_error("Timeline lock failed");
            0
        }
    }
}

// -----------------------------------------------------------------------------
// Media
// -----------------------------------------------------------------------------

unsafe fn store_optional(value: Option<Vec<u8>>, out: *mut *mut u8, len: *mut i32) {
    if out.is_null() || len.is_null() {
        if !len.is_null() {
            *len = 0;
        }
        return;
    }
    match value {
        Some(bytes) => {
            let (pointer, length) = c_buffer(bytes);
            *out = pointer;
            *len = length;
        }
        None => {
            *out = std::ptr::null_mut();
            *len = 0;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pluto_media_inspect(
    path: *const c_char,
    info: *mut PlutoMediaInfo,
    thumbnail: *mut *mut u8,
    thumbnail_len: *mut i32,
    filmstrip: *mut *mut u8,
    filmstrip_len: *mut i32,
    waveform: *mut *mut u8,
    waveform_len: *mut i32,
) -> i32 {
    let path = match read_arg(path) {
        Ok(value) => PathBuf::from(value),
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let probe = match probe_media(&path) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    if !info.is_null() {
        let target = &mut *info;
        write_cstr(&mut target.name, &probe.name);
        write_cstr(&mut target.path, &probe.path);
        write_cstr(&mut target.kind, &probe.kind);
        target.duration = probe.duration;
        target.has_audio = i32::from(probe.has_audio);
    }
    store_optional(
        if probe.kind == "video" { ffmpeg_thumbnail(&path, probe.duration) } else { None },
        thumbnail,
        thumbnail_len,
    );
    store_optional(
        if probe.kind == "video" { ffmpeg_filmstrip(&path, probe.duration) } else { None },
        filmstrip,
        filmstrip_len,
    );
    store_optional(
        if probe.has_audio { ffmpeg_waveform(&path) } else { None },
        waveform,
        waveform_len,
    );
    clear_error();
    1
}

#[no_mangle]
pub unsafe extern "C" fn pluto_media_probe(path: *const c_char, info: *mut PlutoMediaInfo) -> i32 {
    let path = match read_arg(path) {
        Ok(value) => PathBuf::from(value),
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let probe = match probe_media(&path) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    if !info.is_null() {
        let target = &mut *info;
        write_cstr(&mut target.name, &probe.name);
        write_cstr(&mut target.path, &probe.path);
        write_cstr(&mut target.kind, &probe.kind);
        target.duration = probe.duration;
        target.has_audio = i32::from(probe.has_audio);
    }
    clear_error();
    1
}

#[no_mangle]
pub unsafe extern "C" fn pluto_media_frame(
    path: *const c_char,
    time: f64,
    width: i32,
    height: i32,
    out_data: *mut *mut u8,
    out_len: *mut i32,
) -> i32 {
    let path = match read_arg(path) {
        Ok(value) => PathBuf::from(value),
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let frame = ffmpeg_preview_frame(&path, time, width.max(16) as u32, height.max(16) as u32);
    store_optional(frame, out_data, out_len);
    clear_error();
    1
}

#[no_mangle]
pub unsafe extern "C" fn pluto_buffer_free(data: *mut u8, len: i32) {
    if data.is_null() || len <= 0 {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(data, len as usize);
    drop(Box::from_raw(slice));
}

// -----------------------------------------------------------------------------
// Export
// -----------------------------------------------------------------------------

struct ExportState {
    running: bool,
    finished: bool,
    success: bool,
    canceled: bool,
    progress: f64,
    message: String,
    output: String,
    process: ExportProcess,
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            running: false,
            finished: false,
            success: false,
            canceled: false,
            progress: 0.0,
            message: String::new(),
            output: String::new(),
            process: new_export_process(),
        }
    }
}

fn export_state() -> &'static Mutex<ExportState> {
    static EXPORT: OnceLock<Mutex<ExportState>> = OnceLock::new();
    EXPORT.get_or_init(|| Mutex::new(ExportState::default()))
}

#[no_mangle]
pub unsafe extern "C" fn pluto_export_start(settings_json: *const c_char, time: f64, output_path: *const c_char) -> i32 {
    let settings_text = match read_arg(settings_json) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    let settings: Value = match serde_json::from_str(&settings_text) {
        Ok(value) => value,
        Err(error) => {
            set_error(format!("Export settings are not valid JSON: {error}"));
            return 0;
        }
    };
    let output = match read_arg(output_path) {
        Ok(value) => value,
        Err(error) => {
            set_error(error);
            return 0;
        }
    };
    if output.trim().is_empty() {
        set_error("Choose an export destination first");
        return 0;
    }
    let mut guard = match export_state().lock() {
        Ok(value) => value,
        Err(_) => {
            set_error("Export state lock failed");
            return 0;
        }
    };
    if guard.running {
        drop(guard);
        set_error("Another export is already running");
        return 0;
    }
    let clips = match backend().lock() {
        Ok(state) => state
            .timeline
            .clips
            .iter()
            .filter_map(|clip| state.meta.get(&clip.id).map(|meta| export_clip(clip, meta)))
            .collect::<Vec<_>>(),
        Err(_) => {
            drop(guard);
            set_error("Timeline lock failed");
            return 0;
        }
    };
    if clips.is_empty() {
        drop(guard);
        set_error("Add media to the timeline before exporting");
        return 0;
    }
    guard.running = true;
    guard.finished = false;
    guard.success = false;
    guard.canceled = false;
    guard.progress = 0.0;
    guard.message.clear();
    guard.output.clear();
    let process = guard.process.clone();
    drop(guard);
    let progress = Arc::new(move |value: f64| {
        if let Ok(mut state) = export_state().lock() {
            state.progress = value.clamp(0.0, 1.0);
        }
    });
    let output_for_thread = output.clone();
    thread::spawn(move || {
        let result = render_timeline(process, clips, settings, time, PathBuf::from(output_for_thread), progress);
        if let Ok(mut state) = export_state().lock() {
            state.running = false;
            state.finished = true;
            match result {
                Ok(path) => {
                    state.success = true;
                    state.progress = 1.0;
                    state.output = path;
                    state.message.clear();
                }
                Err(error) => {
                    state.success = false;
                    state.message = error;
                }
            }
        }
    });
    clear_error();
    1
}

#[no_mangle]
pub extern "C" fn pluto_export_cancel() -> i32 {
    match export_state().lock() {
        Ok(mut state) => {
            state.canceled = true;
            match state.process.lock() {
                Ok(mut child) => {
                    if let Some(child) = child.as_mut() {
                        let _ = child.kill();
                    }
                    1
                }
                Err(_) => {
                    set_error("Export process lock failed");
                    0
                }
            }
        }
        Err(_) => {
            set_error("Export state lock failed");
            0
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pluto_export_status(out: *mut PlutoExportStatus) -> i32 {
    if out.is_null() {
        set_error("Export status pointer was null");
        return 0;
    }
    match export_state().lock() {
        Ok(state) => {
            let target = &mut *out;
            target.running = i32::from(state.running);
            target.finished = i32::from(state.finished);
            target.success = i32::from(state.success);
            target.canceled = i32::from(state.canceled);
            target.progress = state.progress;
            write_cstr(&mut target.message, &state.message);
            write_cstr(&mut target.output, &state.output);
            clear_error();
            1
        }
        Err(_) => {
            set_error("Export state lock failed");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn pluto_export_running() -> i32 {
    export_state().lock().map(|state| i32::from(state.running)).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn pluto_export_reset() -> i32 {
    match export_state().lock() {
        Ok(mut state) => {
            state.running = false;
            state.finished = false;
            state.success = false;
            state.canceled = false;
            state.progress = 0.0;
            state.message.clear();
            state.output.clear();
            clear_error();
            1
        }
        Err(_) => {
            set_error("Export state lock failed");
            0
        }
    }
}
