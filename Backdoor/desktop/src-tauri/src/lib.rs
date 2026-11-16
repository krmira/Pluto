use pluto_media_engine::{inspect_media, render_timeline, MediaAsset};
use pluto_timeline_core::{Clip, Timeline, TrackId};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, State};
use tauri_plugin_dialog::DialogExt;

type SharedTimeline = Mutex<Timeline>;
type SharedExport = Arc<Mutex<Option<Child>>>;

#[derive(Clone, Debug, Serialize)]
struct ExportProgress {
    progress: f64,
}

#[tauri::command]
async fn import_media(app: tauri::AppHandle) -> Result<Option<MediaAsset>, String> {
    let Some(file) = app.dialog().file().add_filter("Media", &["mp4", "mov", "mkv", "avi", "webm", "m4v", "mp3", "wav", "m4a", "aac", "flac", "ogg", "png", "jpg", "jpeg", "webp", "gif"]).blocking_pick_file() else {
        return Ok(None);
    };
    let path = file.into_path().map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || inspect_media(path))
        .await
        .map_err(|error| error.to_string())?
        .map(Some)
}

#[tauri::command]
async fn choose_export_directory(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let Some(folder) = app.dialog().file().blocking_pick_folder() else {
        return Ok(None);
    };
    Ok(Some(folder.into_path().map_err(|error| error.to_string())?.to_string_lossy().into_owned()))
}

#[tauri::command]
async fn export_timeline(
    app: tauri::AppHandle,
    timeline: State<'_, SharedTimeline>,
    state: State<'_, SharedExport>,
    settings: Value,
    time: f64,
) -> Result<Option<String>, String> {
    let mode = settings["mode"].as_str().unwrap_or("video");
    let format = if mode == "image" {
        settings["image"]["format"].as_str().unwrap_or("png")
    } else {
        settings["video"]["format"].as_str().unwrap_or("mp4")
    };
    let requested_name = settings["output"]["name"].as_str().unwrap_or("pluto-export").trim();
    let suffix = format!(".{format}");
    let name = requested_name.strip_suffix(suffix.as_str()).unwrap_or(requested_name);
    let save = app.dialog().file().add_filter("Export", &[format]).set_file_name(format!(
        "{}.{}",
        if name.is_empty() { "pluto-export" } else { name },
        format
    ));
    let save = if let Some(directory) = settings["output"]["destination"].as_str() {
        save.set_directory(PathBuf::from(directory))
    } else {
        save
    };
    let Some(file) = save.blocking_save_file() else {
        return Ok(None);
    };
    let mut output = file.into_path().map_err(|error| error.to_string())?;
    output.set_extension(format);
    let timeline_clips = timeline.lock().map_err(|_| "Timeline lock failed".to_owned())?.clips.clone();
    let clips = timeline_clips
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let process = state.inner().clone();
    let progress_app = app.clone();
    let progress = Arc::new(move |value: f64| {
        let _ = progress_app.emit("export-progress", ExportProgress { progress: value });
    });
    tauri::async_runtime::spawn_blocking(move || render_timeline(process, clips, settings, time, output, progress))
        .await
        .map_err(|error| error.to_string())?
        .map(Some)
}

#[tauri::command]
fn cancel_export(state: State<'_, SharedExport>) -> Result<(), String> {
    let mut current = state.lock().map_err(|_| "Export process lock failed".to_owned())?;
    if let Some(child) = current.as_mut() {
        child.kill().map_err(|error| format!("Could not cancel export: {error}"))?;
    }
    Ok(())
}

#[tauri::command]
fn timeline_get(state: State<'_, SharedTimeline>) -> Result<Timeline, String> {
    state.lock().map(|timeline| timeline.clone()).map_err(|_| "Timeline lock failed".to_owned())
}

#[tauri::command]
fn timeline_initialize(state: State<'_, SharedTimeline>, clips: Vec<Clip>) -> Result<Timeline, String> {
    let mut timeline = state.lock().map_err(|_| "Timeline lock failed".to_owned())?;
    let snapping = timeline.snapping;
    *timeline = Timeline::new(clips).map_err(|error| format!("{error:?}"))?;
    timeline.snapping = snapping;
    Ok(timeline.clone())
}

#[tauri::command]
fn timeline_add_clip(state: State<'_, SharedTimeline>, clip: Clip) -> Result<Timeline, String> {
    let mut timeline = state.lock().map_err(|_| "Timeline lock failed".to_owned())?;
    timeline.add_clip(clip).map_err(|error| format!("{error:?}"))?;
    Ok(timeline.clone())
}

#[tauri::command]
fn timeline_move_clip(
    state: State<'_, SharedTimeline>,
    id: String,
    start: f64,
    track: TrackId,
    playhead: f64,
    snap_threshold: f64,
) -> Result<Timeline, String> {
    let mut timeline = state.lock().map_err(|_| "Timeline lock failed".to_owned())?;
    timeline.move_clip(&id, start, track, playhead, snap_threshold).map_err(|error| format!("{error:?}"))?;
    Ok(timeline.clone())
}

#[tauri::command]
fn timeline_trim_clip(state: State<'_, SharedTimeline>, id: String, start: f64, end: f64) -> Result<Timeline, String> {
    let mut timeline = state.lock().map_err(|_| "Timeline lock failed".to_owned())?;
    timeline.trim_clip(&id, start, end).map_err(|error| format!("{error:?}"))?;
    Ok(timeline.clone())
}

#[tauri::command]
fn timeline_split_clip(state: State<'_, SharedTimeline>, id: String, at: f64, right_id: String) -> Result<Timeline, String> {
    let mut timeline = state.lock().map_err(|_| "Timeline lock failed".to_owned())?;
    timeline.split_clip(&id, at, right_id).map_err(|error| format!("{error:?}"))?;
    Ok(timeline.clone())
}

#[tauri::command]
fn timeline_delete_clip(state: State<'_, SharedTimeline>, id: String) -> Result<Timeline, String> {
    let mut timeline = state.lock().map_err(|_| "Timeline lock failed".to_owned())?;
    timeline.delete_clip(&id).map_err(|error| format!("{error:?}"))?;
    Ok(timeline.clone())
}

#[tauri::command]
fn timeline_set_snapping(state: State<'_, SharedTimeline>, enabled: bool) -> Result<Timeline, String> {
    let mut timeline = state.lock().map_err(|_| "Timeline lock failed".to_owned())?;
    timeline.snapping = enabled;
    Ok(timeline.clone())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(SharedTimeline::new(Timeline::default()))
        .manage(SharedExport::default())
        .invoke_handler(tauri::generate_handler![
            timeline_get,
            import_media,
            choose_export_directory,
            export_timeline,
            cancel_export,
            timeline_initialize,
            timeline_add_clip,
            timeline_move_clip,
            timeline_trim_clip,
            timeline_split_clip,
            timeline_delete_clip,
            timeline_set_snapping
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Pluto desktop application");
}
