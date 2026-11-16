//! Native Pluto editor shell built with egui.
//!
//! The panel geometry, theme, and icon language follow the existing Frontdoor
//! implementation. Editing state is backed by the shared timeline core, while
//! media preview and export are handled by the shell-independent FFmpeg engine.

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use pluto_timeline_core::{Clip, Timeline, TrackId};
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::PathBuf,
    process::Command,
    hash::{Hash, Hasher},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread,
};

use crate::{icons, theme};

const LOGO_ICO: &[u8] = include_bytes!("../../Icon/icon.ico");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Video,
    Audio,
}

struct Track {
    id: &'static str,
    track_id: TrackId,
    kind: Kind,
    visible: bool,
}

struct MediaItem {
    id: u64,
    path: PathBuf,
    name: String,
    editing_name: bool,
    duration: f64,
    kind: Kind,
    is_still: bool,
    has_audio: bool,
    thumbnail: Option<egui::TextureHandle>,
    waveform: Vec<f32>,
}

struct RawMediaItem {
    id: u64,
    path: PathBuf,
    name: String,
    duration: f64,
    kind: Kind,
    is_still: bool,
    has_audio: bool,
    thumbnail: Option<(usize, usize, Vec<u8>)>,
    waveform: Vec<f32>,
}

#[derive(Clone)]
struct ExportSettings {
    mode: String,
    image_format: String,
    image_quality: f64,
    image_transparent: bool,
    image_metadata: bool,
    image_color_space: String,
    video_format: String,
    video_fps: u32,
    video_quality: String,
    video_audio: bool,
    video_loop: bool,
    width: u32,
    height: u32,
    lock_aspect: bool,
    aspect_ratio: f64,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            mode: "image".to_owned(),
            image_format: "png".to_owned(),
            image_quality: 90.0,
            image_transparent: false,
            image_metadata: true,
            image_color_space: "srgb".to_owned(),
            video_format: "mp4".to_owned(),
            video_fps: 30,
            video_quality: "high".to_owned(),
            video_audio: true,
            video_loop: true,
            width: 1920,
            height: 1080,
            lock_aspect: true,
            aspect_ratio: 16.0 / 9.0,
        }
    }
}

enum ExportEvent {
    Progress(f64),
    Finished(Result<String, String>),
}

enum PreviewCommand {
    Decode { request_id: u64, path: PathBuf, source_time: f64, continuous: bool, loop_image: bool, filter_graph: String },
    Stop,
}

struct PreviewFrame {
    request_id: u64,
    rgba: Vec<u8>,
}

struct KeyframeDrag {
    clip_id: String,
    origin_x: f32,
    pixels_per_second: f32,
    original: Vec<serde_json::Value>,
    indices: HashSet<usize>,
}

struct ClipTrimDrag {
    clip_id: String,
    linked_id: Option<String>,
    left_edge: bool,
    origin_x: f32,
    pixels_per_second: f32,
    start: f64,
    duration: f64,
    source_offset: f64,
}

#[derive(Clone)]
struct ClipDrag {
    primary_id: String,
    origin_x: f32,
    pixels_per_second: f32,
    primary_start: f64,
    primary_track: TrackId,
    clips: Vec<Clip>,
}

const PREVIEW_WIDTH: usize = 640;
const PREVIEW_HEIGHT: usize = 360;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tool {
    Cursor,
    Razor,
    Hand,
    Text,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum InspectorTab {
    Transform,
    Colors,
    Effects,
    Text,
}

pub struct PlutoApp {
    name: String,
    editing_name: bool,
    playing: bool,
    playhead: f64,
    duration: f64,
    project_duration: f64,
    fps: f64,
    zoom: f32,
    tool: Tool,
    magnet: bool,
    selected_track: Option<usize>,
    tracks: Vec<Track>,
    media: Vec<MediaItem>,
    import_commands: Sender<(u64, PathBuf)>,
    import_events: Receiver<(PathBuf, Result<RawMediaItem, String>)>,
    pending_media_paths: HashSet<PathBuf>,
    timeline: Timeline,
    selected_clip: Option<String>,
    selected_clips: HashSet<String>,
    selected_keyframes: HashSet<(String, i64)>,
    keyframe_drag: Option<KeyframeDrag>,
    inspector_tab: InspectorTab,
    last_edit_field: Option<String>,
    dragging_media: Option<u64>,
    timeline_content: Rect,
    track_hit_areas: Vec<(TrackId, Rect)>,
    timeline_offset: f64,
    track_scroll: f32,
    middle_pan_origin: Option<(f32, f64)>,
    hand_pan_origin: Option<(f32, f64)>,
    clip_drag_origin: Option<ClipDrag>,
    clip_trim_drag: Option<ClipTrimDrag>,
    undo: Vec<Timeline>,
    redo: Vec<Timeline>,
    media_search: String,
    show_audio_library: bool,
    media_scroll: f32,
    next_media_id: u64,
    next_clip_id: u64,
    next_group_id: u64,
    show_guides: bool,
    status: Option<String>,
    export_process: pluto_media_engine::SharedExport,
    export_events_tx: Sender<ExportEvent>,
    export_events_rx: Receiver<ExportEvent>,
    export_busy: bool,
    export_progress: f64,
    export_settings: ExportSettings,
    show_export_settings: bool,
    preview_commands: Sender<PreviewCommand>,
    preview_frames: Receiver<PreviewFrame>,
    preview_texture: Option<egui::TextureHandle>,
    preview_request: Option<(String, bool, i64, u64)>,
    preview_request_id: u64,
    stage_rect: Rect,
    text_box_drag: Option<Pos2>,
    title_drag_origin: Option<(String, Pos2, f64, f64)>,
    logo: Option<egui::TextureHandle>,
    startup_paint_requested: bool,
    startup_frame_started: bool,
    startup_frame_finished: bool,
}

impl PlutoApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::install_style(&cc.egui_ctx);

        let logo = load_logo(&cc.egui_ctx);
        let (export_events_tx, export_events_rx) = mpsc::channel();
        let (preview_commands, preview_frames) = start_preview_worker(cc.egui_ctx.clone());
        let (import_commands, import_events) = start_import_worker(cc.egui_ctx.clone());

        let mut tracks = Vec::new();
        for (id, track_id) in [("V4", TrackId::V4), ("V3", TrackId::V3), ("V2", TrackId::V2), ("V1", TrackId::V1)] {
            tracks.push(Track {
                id,
                track_id,
                kind: Kind::Video,
                visible: track_id == TrackId::V1,
            });
        }
        for (id, track_id) in [("A2", TrackId::A2), ("A1", TrackId::A1)] {
            tracks.push(Track {
                id,
                track_id,
                kind: Kind::Audio,
                visible: track_id == TrackId::A1,
            });
        }

        Self {
            name: "Untitled project".to_owned(),
            editing_name: false,
            playing: false,
            playhead: 0.0,
            duration: 60.0,
            project_duration: 0.0,
            fps: 30.0,
            zoom: 1.0,
            tool: Tool::Cursor,
            magnet: true,
            selected_track: None,
            tracks,
            media: Vec::new(),
            import_commands,
            import_events,
            pending_media_paths: HashSet::new(),
            timeline: Timeline::default(),
            selected_clip: None,
            selected_clips: HashSet::new(),
            selected_keyframes: HashSet::new(),
            keyframe_drag: None,
            inspector_tab: InspectorTab::Transform,
            last_edit_field: None,
            dragging_media: None,
            timeline_content: Rect::NOTHING,
            track_hit_areas: Vec::new(),
            timeline_offset: 0.0,
            track_scroll: 0.0,
            middle_pan_origin: None,
            hand_pan_origin: None,
            clip_drag_origin: None,
            clip_trim_drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            media_search: String::new(),
            show_audio_library: false,
            media_scroll: 0.0,
            next_media_id: 1,
            next_clip_id: 1,
            next_group_id: 1,
            show_guides: false,
            status: None,
            export_process: pluto_media_engine::SharedExport::default(),
            export_events_tx,
            export_events_rx,
            export_busy: false,
            export_progress: 0.0,
            export_settings: ExportSettings::default(),
            show_export_settings: false,
            preview_commands,
            preview_frames,
            preview_texture: None,
            preview_request: None,
            preview_request_id: 0,
            stage_rect: Rect::NOTHING,
            text_box_drag: None,
            title_drag_origin: None,
            logo,
            startup_paint_requested: false,
            startup_frame_started: false,
            startup_frame_finished: false,
        }
    }

    fn tick(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.pointer.any_released()) {
            self.last_edit_field = None;
        }
        self.handle_shortcuts(ctx);
        self.finish_media_drop(ctx);
        self.poll_import_events(ctx);
        self.poll_export_events();
        self.refresh_timeline_duration();
        if self.playing {
            let dt = ctx.input(|i| i.stable_dt).min(0.1) as f64;
            self.playhead += dt;
            if self.playhead >= self.project_duration {
                self.playhead = self.project_duration;
                self.playing = false;
            }
            ctx.request_repaint();
        }
    }

    fn refresh_timeline_duration(&mut self) {
        self.project_duration = self.timeline.clips().iter().map(Clip::end).fold(0.0, f64::max);
        self.duration = self.timeline.duration_seconds.max(60.0);
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (save_project, open_project, space, left, right, delete, undo, redo, home, end, add_keyframe, previous_keyframe, next_keyframe, cursor_tool, hand_tool, razor_tool, text_tool) = ctx.input(|input| (
            input.modifiers.command && input.key_pressed(egui::Key::S),
            input.modifiers.command && input.key_pressed(egui::Key::O),
            input.key_pressed(egui::Key::Space),
            input.key_pressed(egui::Key::ArrowLeft),
            input.key_pressed(egui::Key::ArrowRight),
            input.key_pressed(egui::Key::Delete) || input.key_pressed(egui::Key::Backspace),
            input.modifiers.command && input.key_pressed(egui::Key::Z) && !input.modifiers.shift,
            input.modifiers.command && (input.key_pressed(egui::Key::Y) || (input.key_pressed(egui::Key::Z) && input.modifiers.shift)),
            input.key_pressed(egui::Key::Home),
            input.key_pressed(egui::Key::End),
            input.key_pressed(egui::Key::K),
            input.modifiers.alt && input.key_pressed(egui::Key::ArrowLeft),
            input.modifiers.alt && input.key_pressed(egui::Key::ArrowRight),
            input.key_pressed(egui::Key::V) && !input.modifiers.command,
            input.key_pressed(egui::Key::H) && !input.modifiers.command,
            input.key_pressed(egui::Key::C) && !input.modifiers.command,
            input.key_pressed(egui::Key::T) && !input.modifiers.command,
        ));
        // Context accessors lock egui's shared state; keep this outside the
        // ctx.input closure to avoid recursively locking it during the first UI frame.
        let text_focus = ctx.egui_wants_keyboard_input();
        if text_focus { return; }
        if save_project { self.save_project(); }
        if open_project { self.open_project(); }
        if space {
            self.playing = !self.playing;
            if self.playing && self.playhead >= self.project_duration { self.playhead = 0.0; }
        }
        let frame = 1.0 / self.fps.max(1.0);
        if left { self.playing = false; self.playhead = (self.playhead - frame).max(0.0); }
        if right { self.playing = false; self.playhead = (self.playhead + frame).min(self.project_duration); }
        if home { self.playhead = 0.0; }
        if end { self.playhead = self.project_duration; }
        if add_keyframe { self.toggle_keyframe(); }
        if previous_keyframe { self.seek_keyframe(false); }
        if next_keyframe { self.seek_keyframe(true); }
        if cursor_tool { self.tool = Tool::Cursor; }
        if hand_tool { self.tool = Tool::Hand; }
        if razor_tool { self.tool = Tool::Razor; }
        if text_tool { self.tool = Tool::Text; self.inspector_tab = InspectorTab::Text; }
        if delete { self.delete_selected_clip(); }
        if undo { self.undo_timeline(); }
        if redo { self.redo_timeline(); }
    }

    fn delete_selected_clip(&mut self) {
        if !self.selected_keyframes.is_empty() {
            let Some(id) = self.selected_clip.clone() else { self.selected_keyframes.clear(); return };
            let selected: HashSet<i64> = self.selected_keyframes.iter().filter(|(clip_id, _)| clip_id == &id).map(|(_, frame)| *frame).collect();
            let mut clips = self.timeline.clips().to_vec();
            let changed = if let Some(clip) = clips.iter_mut().find(|clip| clip.id == id) {
                if let Some(keys) = clip.extra.get_mut("keyframes").and_then(serde_json::Value::as_array_mut) {
                    keys.retain(|key| !key["time"].as_f64().is_some_and(|time| selected.contains(&((time * self.fps.max(1.0)).round() as i64))));
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if changed {
                if let Ok(mut timeline) = Timeline::new(clips) {
                    self.push_undo();
                    timeline.snapping = self.magnet;
                    self.timeline = timeline;
                    self.selected_keyframes.clear();
                }
            }
            return;
        }
        let mut ids = self.selected_clip_ids();
        if ids.is_empty() { return; }
        self.push_undo();
        for id in ids.drain() {
            if self.timeline.clips().iter().any(|clip| clip.id == id) {
                let _ = self.timeline.delete_clip(&id);
            }
        }
        self.selected_clip = None;
        self.selected_clips.clear();
        self.selected_keyframes.clear();
        self.refresh_timeline_duration();
    }

    fn selected_clip_ids(&self) -> HashSet<String> {
        let mut ids = self.selected_clips.clone();
        if ids.is_empty() {
            if let Some(id) = self.selected_clip.as_ref() { ids.insert(id.clone()); }
        }
        let group_ids: HashSet<String> = self.timeline.clips().iter()
            .filter(|clip| ids.contains(&clip.id))
            .filter_map(|clip| clip.extra.get("groupId").and_then(serde_json::Value::as_str).map(str::to_owned))
            .collect();
        if !group_ids.is_empty() {
            ids.extend(self.timeline.clips().iter().filter(|clip| clip.extra.get("groupId").and_then(serde_json::Value::as_str).is_some_and(|group| group_ids.contains(group))).map(|clip| clip.id.clone()));
        }
        let linked: Vec<String> = self.timeline.clips().iter().filter(|clip| ids.contains(&clip.id)).filter_map(|clip| clip.linked_clip_id.clone()).collect();
        ids.extend(linked);
        ids
    }

    fn toggle_selected_group(&mut self) {
        let mut ids = self.selected_clip_ids();
        if ids.len() < 2 { return; }
        let selected_groups: Vec<Option<String>> = self.timeline.clips().iter().filter(|clip| ids.contains(&clip.id))
            .map(|clip| clip.extra.get("groupId").and_then(serde_json::Value::as_str).map(str::to_owned)).collect();
        let existing = selected_groups.first().cloned().flatten();
        let ungroup = existing.is_some() && selected_groups.iter().all(|group| group.as_ref() == existing.as_ref());
        if ungroup {
            if let Some(group_id) = existing {
                ids.extend(self.timeline.clips().iter().filter(|clip| clip.extra.get("groupId").and_then(serde_json::Value::as_str) == Some(group_id.as_str())).map(|clip| clip.id.clone()));
            }
        }
        let mut clips = self.timeline.clips().to_vec();
        if ungroup {
            for clip in &mut clips { if ids.contains(&clip.id) { clip.extra.remove("groupId"); } }
        } else {
            let group_id = format!("group-{}", self.next_group_id);
            self.next_group_id += 1;
            for clip in &mut clips { if ids.contains(&clip.id) { clip.extra.insert("groupId".to_owned(), serde_json::json!(group_id)); } }
        }
        if let Ok(mut timeline) = Timeline::new(clips) {
            self.push_undo();
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn split_clip_at(&mut self, id: &str, at: f64) {
        let Some(original) = self.timeline.clips().iter().find(|clip| clip.id == id).cloned() else { return };
        let right_id = format!("clip-{}", self.next_clip_id);
        let linked_right_id = format!("clip-{}", self.next_clip_id + 1);
        let mut timeline = self.timeline.clone();
        if timeline.split_clip(id, at, right_id.clone()).is_err() { return; }
        let mut split_linked = false;
        if let Some(linked_id) = original.linked_clip_id.as_deref() {
            if timeline.clips().iter().any(|clip| clip.id == linked_id) {
                if timeline.split_clip(linked_id, at, linked_right_id.clone()).is_err() { return; }
                let mut clips = timeline.clips().to_vec();
                let right_video_id = if original.track.is_video() { right_id.clone() } else { linked_right_id.clone() };
                let right_audio_id = if original.track.is_video() { linked_right_id.clone() } else { right_id.clone() };
                if let Some(video) = clips.iter_mut().find(|clip| clip.id == right_video_id) {
                    video.linked_clip_id = Some(right_audio_id.clone());
                }
                if let Some(audio) = clips.iter_mut().find(|clip| clip.id == right_audio_id) {
                    audio.linked_clip_id = Some(right_video_id);
                }
                let Ok(mut rebuilt) = Timeline::new(clips) else { return };
                rebuilt.snapping = self.magnet;
                timeline = rebuilt;
                split_linked = true;
            }
        }
        self.push_undo();
        timeline.snapping = self.magnet;
        self.timeline = timeline;
        self.next_clip_id += if split_linked { 2 } else { 1 };
        self.refresh_timeline_duration();
    }

    fn separate_audio(&mut self, id: &str) {
        let Some(video) = self.timeline.clips().iter().find(|clip| clip.id == id).cloned() else { return };
        if !video.track.is_video()
            || video.extra.get("hasAudio").and_then(serde_json::Value::as_bool) != Some(true)
            || video.linked_clip_id.is_some()
        {
            return;
        }
        let audio_id = format!("clip-{}", self.next_clip_id);
        let mut clips = self.timeline.clips().to_vec();
        let Some(video_mut) = clips.iter_mut().find(|clip| clip.id == id) else { return };
        video_mut.linked_clip_id = Some(audio_id.clone());
        video_mut.extra.insert("audioDetached".to_owned(), serde_json::json!(false));

        let mut audio = video;
        audio.id = audio_id;
        audio.track = if clips.iter().any(|clip| clip.track == TrackId::A1 && clip.start < audio.end() && clip.end() > audio.start) { TrackId::A2 } else { TrackId::A1 };
        audio.label = format!("{} (Audio)", audio.label);
        audio.linked_clip_id = Some(id.to_owned());
        audio.extra.insert("kind".to_owned(), serde_json::json!("audio"));
        audio.extra.insert("audioDetached".to_owned(), serde_json::json!(true));
        clips.push(audio);
        match Timeline::new(clips) {
            Ok(mut timeline) => {
                self.push_undo();
                timeline.snapping = self.magnet;
                self.timeline = timeline;
                self.next_clip_id += 1;
                self.refresh_timeline_duration();
                self.status = Some("Audio separated and linked to its video clip".to_owned());
            }
            Err(error) => self.status = Some(format!("Could not separate audio: {error:?}")),
        }
    }

    fn set_clip_locked(&mut self, id: &str, locked: bool) {
        let mut clips = self.timeline.clips().to_vec();
        let Some(clip) = clips.iter_mut().find(|clip| clip.id == id) else { return };
        if clip.extra.get("locked").and_then(serde_json::Value::as_bool) == Some(locked) { return; }
        clip.extra.insert("locked".to_owned(), serde_json::json!(locked));
        if let Ok(mut timeline) = Timeline::new(clips) {
            self.push_undo();
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn unlink_audio(&mut self, id: &str) {
        let mut clips = self.timeline.clips().to_vec();
        let Some(index) = clips.iter().position(|clip| clip.id == id) else { return };
        let linked_id = clips[index].linked_clip_id.clone();
        let Some(linked_id) = linked_id else { return };
        clips[index].linked_clip_id = None;
        if let Some(linked) = clips.iter_mut().find(|clip| clip.id == linked_id) {
            linked.linked_clip_id = None;
            if linked.track.is_video() { linked.extra.insert("audioDetached".to_owned(), serde_json::json!(true)); }
        }
        if let Ok(mut timeline) = Timeline::new(clips) {
            self.push_undo();
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn selected_property(&self, path: &[&str]) -> Option<&serde_json::Value> {
        let id = self.selected_clip.as_deref()?;
        let clip = self.timeline.clips().iter().find(|clip| clip.id == id)?;
        let local_time = (self.playhead - clip.start).clamp(0.0, clip.duration);
        active_keyframe_index(clip, local_time, self.fps)
            .and_then(|index| clip.extra.get("keyframes")?.get(index)?.get("props"))
            .and_then(|props| json_at_path(props, path))
            .or_else(|| clip.extra.get("props").and_then(|props| json_at_path(props, path)))
    }

    fn property_number(&self, path: &[&str], fallback: f64) -> f64 {
        let Some(id) = self.selected_clip.as_deref() else { return fallback };
        let Some(clip) = self.timeline.clips().iter().find(|clip| clip.id == id) else { return fallback };
        let base = clip.extra.get("props").and_then(|props| json_at_path(props, path)).and_then(|value| value.as_f64()).unwrap_or(fallback);
        let mut points = vec![(0.0_f64, base, "linear".to_owned())];
        if let Some(keyframes) = clip.extra.get("keyframes").and_then(|value| value.as_array()) {
            for keyframe in keyframes {
                let Some(time) = keyframe["time"].as_f64().filter(|time| time.is_finite()) else { continue };
                let Some(value) = keyframe.get("props").and_then(|props| json_at_path(props, path)).and_then(|value| value.as_f64()).filter(|value| value.is_finite()) else { continue };
                let point = (time, value, keyframe["easing"].as_str().unwrap_or("linear").to_owned());
                if let Some(existing) = points.iter_mut().find(|existing| (existing.0 - time).abs() < 1e-6) { *existing = point; }
                else { points.push(point); }
            }
        }
        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let time = (self.playhead - clip.start).clamp(0.0, clip.duration);
        if time <= points[0].0 { return points[0].1; }
        for pair in points.windows(2) {
            if time <= pair[1].0 {
                if pair[1].0 <= pair[0].0 { return pair[1].1; }
                let mut progress = ((time - pair[0].0) / (pair[1].0 - pair[0].0)).clamp(0.0, 1.0);
                progress = match pair[0].2.as_str() {
                    "ease-in" => progress * progress,
                    "ease-out" => 1.0 - (1.0 - progress) * (1.0 - progress),
                    "ease-in-out" => if progress < 0.5 { 2.0 * progress * progress } else { 1.0 - 2.0 * (1.0 - progress) * (1.0 - progress) },
                    "hold" => 0.0,
                    _ => progress,
                };
                return pair[0].1 + (pair[1].1 - pair[0].1) * progress;
            }
        }
        points.last().map(|point| point.1).unwrap_or(fallback)
    }

    fn property_bool(&self, path: &[&str], fallback: bool) -> bool {
        self.selected_property(path).and_then(|value| value.as_bool()).unwrap_or(fallback)
    }

    fn property_string(&self, path: &[&str], fallback: &str) -> String {
        self.selected_property(path).and_then(|value| value.as_str()).unwrap_or(fallback).to_owned()
    }

    fn set_property_value(&mut self, path: &[&str], value: serde_json::Value) {
        let Some(id) = self.selected_clip.clone() else { return };
        if self.selected_property(path) == Some(&value) { return; }
        let edit_key = format!("{}:{}", id, path.join("."));
        if self.last_edit_field.as_deref() != Some(edit_key.as_str()) {
            self.push_undo();
            self.last_edit_field = Some(edit_key);
        }

        let mut clips = self.timeline.clips().to_vec();
        let Some(clip) = clips.iter_mut().find(|clip| clip.id == id) else { return };
        let local_time = (self.playhead - clip.start).clamp(0.0, clip.duration);
        let props = clip_property_store_mut(clip, local_time, self.fps);
        set_json_path(props, path, value);
        if let Ok(mut timeline) = Timeline::new(clips) {
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn set_clip_extra_field(&mut self, field: &str, value: Option<serde_json::Value>) {
        let Some(id) = self.selected_clip.clone() else { return };
        let mut clips = self.timeline.clips().to_vec();
        let Some(clip) = clips.iter_mut().find(|clip| clip.id == id) else { return };
        let unchanged = match &value {
            Some(value) => clip.extra.get(field) == Some(value),
            None => !clip.extra.contains_key(field),
        };
        if unchanged { return; }
        let key = format!("{id}:{field}");
        if self.last_edit_field.as_deref() != Some(key.as_str()) {
            self.push_undo();
            self.last_edit_field = Some(key);
        }
        if let Some(value) = value { clip.extra.insert(field.to_owned(), value); }
        else { clip.extra.remove(field); }
        if let Ok(mut timeline) = Timeline::new(clips) {
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn set_transition(&mut self, kind: &str, duration: f64) {
        if kind == "none" {
            self.set_clip_extra_field("trans", None);
        } else {
            self.set_clip_extra_field("trans", Some(serde_json::json!({ "type": kind, "dur": duration.clamp(0.05, 5.0) })));
        }
    }

    fn set_title_position(&mut self, id: &str, x: f64, y: f64) {
        let key = format!("{id}:transform.position");
        if self.last_edit_field.as_deref() != Some(key.as_str()) {
            self.push_undo();
            self.last_edit_field = Some(key);
        }
        let mut clips = self.timeline.clips().to_vec();
        let Some(clip) = clips.iter_mut().find(|clip| clip.id == id) else { return };
        let local_time = (self.playhead - clip.start).clamp(0.0, clip.duration);
        let props = clip_property_store_mut(clip, local_time, self.fps);
        set_json_path(props, &["transform", "position", "x"], serde_json::json!(x));
        set_json_path(props, &["transform", "position", "y"], serde_json::json!(y));
        if let Ok(mut timeline) = Timeline::new(clips) {
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn reset_transform_group(&mut self, group: &str) {
        let Some(id) = self.selected_clip.clone() else { return };
        let unchanged = match group {
            "position" => self.property_number(&["transform", "position", "x"], 0.0).abs() < f64::EPSILON
                && self.property_number(&["transform", "position", "y"], 0.0).abs() < f64::EPSILON,
            "scale" => (self.property_number(&["transform", "scale", "w"], 100.0) - 100.0).abs() < f64::EPSILON
                && (self.property_number(&["transform", "scale", "h"], 100.0) - 100.0).abs() < f64::EPSILON
                && self.property_bool(&["transform", "scale", "linked"], true),
            _ => true,
        };
        if unchanged { return; }
        let mut clips = self.timeline.clips().to_vec();
        let Some(clip) = clips.iter_mut().find(|clip| clip.id == id) else { return };
        let key = format!("{id}:transform.{group}.reset");
        if self.last_edit_field.as_deref() != Some(key.as_str()) {
            self.push_undo();
            self.last_edit_field = Some(key);
        }
        let local_time = (self.playhead - clip.start).clamp(0.0, clip.duration);
        let props = clip_property_store_mut(clip, local_time, self.fps);
        match group {
            "position" => {
                set_json_path(props, &["transform", "position", "x"], serde_json::json!(0.0));
                set_json_path(props, &["transform", "position", "y"], serde_json::json!(0.0));
            }
            "scale" => {
                set_json_path(props, &["transform", "scale", "w"], serde_json::json!(100.0));
                set_json_path(props, &["transform", "scale", "h"], serde_json::json!(100.0));
                set_json_path(props, &["transform", "scale", "linked"], serde_json::json!(true));
            }
            _ => return,
        }
        if let Ok(mut timeline) = Timeline::new(clips) {
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn set_number(&mut self, path: &[&str], value: f64) {
        self.set_property_value(path, serde_json::json!(value));
    }

    fn set_scale_axis(&mut self, axis: &str, value: f64) {
        let linked = self.property_bool(&["transform", "scale", "linked"], true);
        let old_axis = self.property_number(&["transform", "scale", axis], 100.0);
        let other = if axis == "w" { "h" } else { "w" };
        let old_other = self.property_number(&["transform", "scale", other], 100.0);
        let linked_value = if old_axis.abs() > f64::EPSILON { old_other * value / old_axis } else { value };
        let mut clips = self.timeline.clips().to_vec();
        let Some(id) = self.selected_clip.as_deref() else { return };
        let Some(clip) = clips.iter_mut().find(|clip| clip.id == id) else { return };
        let key = format!("{}:transform.scale.{axis}", id);
        if self.last_edit_field.as_deref() != Some(key.as_str()) {
            self.push_undo();
            self.last_edit_field = Some(key);
        }
        let local_time = (self.playhead - clip.start).clamp(0.0, clip.duration);
        let props = clip_property_store_mut(clip, local_time, self.fps);
        set_json_path(props, &["transform", "scale", axis], serde_json::json!(value));
        if linked {
            set_json_path(props, &["transform", "scale", other], serde_json::json!(linked_value));
        }
        if let Ok(mut timeline) = Timeline::new(clips) {
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn poll_export_events(&mut self) {
        loop {
            match self.export_events_rx.try_recv() {
                Ok(ExportEvent::Progress(progress)) => self.export_progress = progress.clamp(0.0, 1.0),
                Ok(ExportEvent::Finished(Ok(path))) => {
                    self.export_busy = false;
                    self.export_progress = 1.0;
                    self.status = Some(format!("Export complete: {path}"));
                }
                Ok(ExportEvent::Finished(Err(error))) => {
                    self.export_busy = false;
                    self.status = Some(format!("Export failed: {error}"));
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    fn start_or_cancel_export(&mut self) {
        if self.export_busy {
            if let Err(error) = pluto_media_engine::cancel_export(&self.export_process) {
                self.status = Some(error);
            }
        } else {
            self.show_export_settings = true;
        }
    }

    fn export_settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_export_settings;
        let mut begin_export = false;
        let mut close_window = false;
        egui::Window::new("Export")
            .id(egui::Id::new("pluto-export-settings"))
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("Output type").color(theme::MUTED));
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.export_settings.mode, "video".to_owned(), "Video");
                    ui.selectable_value(&mut self.export_settings.mode, "image".to_owned(), "Image");
                });
                ui.separator();
                ui.label(egui::RichText::new("Format").color(theme::MUTED));
                if self.export_settings.mode == "image" {
                    egui::ComboBox::from_id_salt("pluto-export-image-format")
                        .selected_text(self.export_settings.image_format.to_uppercase())
                        .show_ui(ui, |ui| {
                            for format in ["png", "jpg", "webp", "avif"] {
                                ui.selectable_value(&mut self.export_settings.image_format, format.to_owned(), format.to_uppercase());
                            }
                        });
                    if self.export_settings.image_format != "png" {
                        ui.add(egui::Slider::new(&mut self.export_settings.image_quality, 1.0..=100.0).text("Image quality"));
                    }
                    if self.export_settings.image_format != "jpg" {
                        ui.checkbox(&mut self.export_settings.image_transparent, "Transparent background");
                    } else {
                        self.export_settings.image_transparent = false;
                    }
                    ui.checkbox(&mut self.export_settings.image_metadata, "Include metadata");
                    ui.horizontal(|ui| {
                        ui.label("Color space");
                        ui.selectable_value(&mut self.export_settings.image_color_space, "srgb".to_owned(), "sRGB");
                        ui.selectable_value(&mut self.export_settings.image_color_space, "p3".to_owned(), "Display P3");
                    });
                } else {
                    egui::ComboBox::from_id_salt("pluto-export-video-format")
                        .selected_text(self.export_settings.video_format.to_uppercase())
                        .show_ui(ui, |ui| {
                            for format in ["mp4", "webm", "gif"] {
                                ui.selectable_value(&mut self.export_settings.video_format, format.to_owned(), format.to_uppercase());
                            }
                        });
                    ui.horizontal(|ui| {
                        ui.label("Frame rate");
                        egui::ComboBox::from_id_salt("pluto-export-fps")
                            .selected_text(format!("{} fps", self.export_settings.video_fps))
                            .show_ui(ui, |ui| {
                                for fps in [24, 30, 60] {
                                    ui.selectable_value(&mut self.export_settings.video_fps, fps, format!("{fps} fps"));
                                }
                            });
                    });
                    if self.export_settings.video_format != "gif" {
                        egui::ComboBox::from_id_salt("pluto-export-video-quality")
                            .selected_text(format!("Quality: {}", self.export_settings.video_quality))
                            .show_ui(ui, |ui| {
                                for quality in ["low", "medium", "high", "max"] {
                                    ui.selectable_value(&mut self.export_settings.video_quality, quality.to_owned(), quality.to_uppercase());
                                }
                            });
                        ui.checkbox(&mut self.export_settings.video_audio, "Include audio");
                    } else {
                        ui.checkbox(&mut self.export_settings.video_loop, "Loop forever");
                    }
                }
                ui.separator();
                ui.label(egui::RichText::new("Output size").color(theme::MUTED));
                let mut width = self.export_settings.width;
                let mut height = self.export_settings.height;
                let mut lock_aspect = self.export_settings.lock_aspect;
                let mut aspect_ratio = self.export_settings.aspect_ratio.max(0.01);
                let mut width_changed = false;
                let mut height_changed = false;
                let mut lock_changed = false;
                ui.horizontal(|ui| {
                    ui.label("Width");
                    width_changed = ui.add(egui::DragValue::new(&mut width).range(16..=7680).suffix(" px")).changed();
                    lock_changed = ui.checkbox(&mut lock_aspect, "Lock ratio").changed();
                    ui.label("Height");
                    height_changed = ui.add(egui::DragValue::new(&mut height).range(16..=4320).suffix(" px")).changed();
                });
                if lock_changed && lock_aspect {
                    aspect_ratio = width as f64 / height.max(1) as f64;
                }
                if lock_aspect {
                    if width_changed {
                        height = ((width as f64 / aspect_ratio).round() as u32).clamp(16, 4320);
                    } else if height_changed {
                        width = ((height as f64 * aspect_ratio).round() as u32).clamp(16, 7680);
                    }
                }
                self.export_settings.width = width.clamp(16, 7680);
                self.export_settings.height = height.clamp(16, 4320);
                self.export_settings.lock_aspect = lock_aspect;
                self.export_settings.aspect_ratio = aspect_ratio;
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() { close_window = true; }
                    if ui.button(egui::RichText::new("Export").strong()).clicked() { begin_export = true; }
                });
            });
        if close_window { open = false; }
        self.show_export_settings = open;
        if begin_export {
            self.show_export_settings = false;
            self.start_export();
        }
    }

    fn start_export(&mut self) {
        if self.timeline.clips().is_empty() {
            self.status = Some("Add media to the timeline before exporting".to_owned());
            return;
        }
        let settings = self.export_settings.clone();
        let (extension, filter_name, filter_extensions) = if settings.mode == "image" {
            (settings.image_format.clone(), "Image", vec![settings.image_format.as_str()])
        } else {
            (settings.video_format.clone(), "Video", vec![settings.video_format.as_str()])
        };
        let file_name = format!("{}.{}", safe_filename(&self.name), extension);
        let mut dialog = rfd::FileDialog::new().add_filter(filter_name, &filter_extensions);
        dialog = dialog.set_file_name(file_name);
        let Some(output) = dialog.save_file() else { return };

        let clips = match self.timeline.clips().iter().map(serde_json::to_value).collect::<Result<Vec<_>, _>>() {
            Ok(clips) => clips,
            Err(error) => { self.status = Some(format!("Could not prepare export: {error}")); return; }
        };
        let engine_settings = serde_json::json!({
            "mode": settings.mode.clone(),
            "size": { "width": settings.width, "height": settings.height },
            "video": {
                "fps": settings.video_fps,
                "format": settings.video_format.clone(),
                "quality": settings.video_quality.clone(),
                "audio": settings.video_audio,
                "loop": settings.video_loop
            },
            "image": {
                "format": settings.image_format.clone(),
                "quality": settings.image_quality,
                "transparent": settings.image_transparent,
                "metadata": settings.image_metadata,
                "colorSpace": settings.image_color_space
            },
            "output": { "name": self.name }
        });
        let output_time = if settings.mode == "image" {
            self.playhead.clamp(0.0, (self.project_duration - 1.0 / self.fps.max(1.0)).max(0.0))
        } else {
            0.0
        };
        let events = self.export_events_tx.clone();
        let process = self.export_process.clone();
        let (progress_tx, progress_rx) = mpsc::channel();
        let progress_events = events.clone();
        thread::spawn(move || {
            for progress in progress_rx {
                if progress_events.send(ExportEvent::Progress(progress)).is_err() { break; }
            }
        });
        self.export_busy = true;
        self.export_progress = 0.0;
        self.status = Some(if settings.mode == "image" { "Exporting frame…" } else { "Exporting video…" }.to_owned());
        thread::spawn(move || {
            let result = pluto_media_engine::render_timeline(process, clips, engine_settings, output_time, output, progress_tx);
            let _ = events.send(ExportEvent::Finished(result));
        });
    }

    fn toggle_keyframe(&mut self) {
        let Some(id) = self.selected_clip.clone() else { return };
        let Some(snapshot) = self.timeline.clips().iter().find(|clip| clip.id == id) else { return };
        let local_time = ((self.playhead - snapshot.start).clamp(0.0, snapshot.duration) * self.fps.max(1.0)).round() / self.fps.max(1.0);
        let mut current_props = snapshot.extra.get("props").cloned().unwrap_or_else(|| serde_json::json!({}));
        let numeric_paths: &[(&[&str], f64)] = &[
            (&["transform", "position", "x"], 0.0), (&["transform", "position", "y"], 0.0),
            (&["transform", "scale", "w"], 100.0), (&["transform", "scale", "h"], 100.0),
            (&["transform", "rotation"], 0.0), (&["transform", "opacity"], 100.0),
            (&["colors", "light", "exposure"], 0.0), (&["colors", "light", "brightness"], 0.0), (&["colors", "light", "contrast"], 0.0),
            (&["colors", "color", "saturation"], 0.0), (&["colors", "color", "vibrance"], 0.0), (&["colors", "color", "temperature"], 0.0),
            (&["colors", "detail", "clarity"], 0.0), (&["colors", "detail", "sharpness"], 0.0), (&["colors", "detail", "vignette"], 0.0),
            (&["text", "size"], 100.0), (&["audio", "volume"], 100.0), (&["audio", "pan"], 0.0),
            (&["audio", "bass"], 0.0), (&["audio", "treble"], 0.0),
        ];
        for (path, fallback) in numeric_paths {
            set_json_path(&mut current_props, path, serde_json::json!(self.property_number(path, *fallback)));
        }
        let mut clips = self.timeline.clips().to_vec();
        let Some(clip) = clips.iter_mut().find(|clip| clip.id == id) else { return };
        let keyframes = clip.extra.entry("keyframes".to_owned()).or_insert_with(|| serde_json::json!([]));
        let Some(keyframes) = keyframes.as_array_mut() else { return };
        self.push_undo();
        if let Some(index) = keyframes.iter().position(|keyframe| (keyframe["time"].as_f64().unwrap_or(-10.0) - local_time).abs() < 1.0 / self.fps.max(1.0)) {
            keyframes.remove(index);
            self.selected_keyframes.remove(&(id.clone(), (local_time * self.fps.max(1.0)).round() as i64));
        } else {
            keyframes.push(serde_json::json!({ "time": local_time, "props": current_props, "easing": "linear" }));
            keyframes.sort_by(|a, b| a["time"].as_f64().unwrap_or(0.0).partial_cmp(&b["time"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal));
            self.selected_keyframes.clear();
            self.selected_keyframes.insert((id.clone(), (local_time * self.fps.max(1.0)).round() as i64));
        }
        if let Ok(mut timeline) = Timeline::new(clips) {
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn seek_keyframe(&mut self, next: bool) {
        let Some(id) = self.selected_clip.as_deref() else { return };
        let Some(clip) = self.timeline.clips().iter().find(|clip| clip.id == id) else { return };
        let local_time = (self.playhead - clip.start).clamp(0.0, clip.duration);
        let tolerance = 0.5 / self.fps.max(1.0);
        let mut keyframes: Vec<f64> = clip.extra.get("keyframes").and_then(serde_json::Value::as_array)
            .into_iter().flatten().filter_map(|keyframe| keyframe["time"].as_f64())
            .filter(|time| time.is_finite() && *time >= 0.0 && *time <= clip.duration)
            .collect();
        keyframes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let target = if next {
            keyframes.into_iter().find(|time| *time > local_time + tolerance)
        } else {
            keyframes.into_iter().rev().find(|time| *time < local_time - tolerance)
        };
        if let Some(target) = target {
            self.playing = false;
            self.playhead = clip.start + target;
            self.selected_keyframes.clear();
            self.selected_keyframes.insert((clip.id.clone(), (target * self.fps.max(1.0)).round() as i64));
        }
    }

    fn set_keyframe_easing(&mut self, clip_id: &str, index: usize, easing: &str) {
        let mut clips = self.timeline.clips().to_vec();
        let Some(clip) = clips.iter_mut().find(|clip| clip.id == clip_id) else { return };
        let Some(keyframes) = clip.extra.get_mut("keyframes").and_then(serde_json::Value::as_array_mut) else { return };
        let selected: HashSet<i64> = self.selected_keyframes.iter().filter(|(id, _)| id == clip_id).map(|(_, time)| *time).collect();
        let fps = self.fps.max(1.0);
        for (keyframe_index, keyframe) in keyframes.iter_mut().enumerate() {
            let time_key = (keyframe["time"].as_f64().unwrap_or(0.0) * fps).round() as i64;
            if keyframe_index == index || selected.contains(&time_key) {
                keyframe["easing"] = serde_json::json!(easing);
            }
        }
        if let Ok(mut timeline) = Timeline::new(clips) {
            self.push_undo();
            timeline.snapping = self.magnet;
            self.timeline = timeline;
        }
    }

    fn apply_keyframe_drag(&mut self, pointer_x: f32) {
        let Some(drag) = self.keyframe_drag.as_ref() else { return };
        let clip_id = drag.clip_id.clone();
        let origin_x = drag.origin_x;
        let pixels_per_second = drag.pixels_per_second.max(0.001);
        let original = drag.original.clone();
        let indices = drag.indices.clone();
        let mut delta = (pointer_x - origin_x) as f64 / pixels_per_second as f64;
        let fps = self.fps.max(1.0);
        let mut clips = self.timeline.clips().to_vec();
        let Some(clip) = clips.iter_mut().find(|clip| clip.id == clip_id) else { return };
        let frame = 1.0 / fps;
        let selected_times: Vec<f64> = original.iter().enumerate().filter_map(|(index, keyframe)| {
            indices.contains(&index).then(|| keyframe["time"].as_f64()).flatten()
        }).collect();
        let other_times: Vec<f64> = original.iter().enumerate().filter_map(|(index, keyframe)| {
            (!indices.contains(&index)).then(|| keyframe["time"].as_f64()).flatten()
        }).collect();
        if let (Some(min_selected), Some(max_selected)) = (selected_times.iter().copied().reduce(f64::min), selected_times.iter().copied().reduce(f64::max)) {
            let lower = other_times.iter().copied().filter(|time| *time < min_selected).map(|time| time + frame - min_selected).fold(-min_selected, f64::max);
            let upper = other_times.iter().copied().filter(|time| *time > max_selected).map(|time| time - frame - max_selected).fold(clip.duration - max_selected, f64::min);
            delta = delta.clamp(lower, upper.max(lower));
        }
        let mut selected = HashSet::new();
        let updated: Vec<serde_json::Value> = original.into_iter().enumerate().map(|(index, mut keyframe)| {
            if indices.contains(&index) {
                let time = (keyframe["time"].as_f64().unwrap_or(0.0) + delta).clamp(0.0, clip.duration);
                let frame_time = (time * fps).round() / fps;
                keyframe["time"] = serde_json::json!(frame_time);
                selected.insert((clip_id.clone(), (frame_time * fps).round() as i64));
            }
            keyframe
        }).collect();
        clip.extra.insert("keyframes".to_owned(), serde_json::Value::Array(updated));
        if let Ok(mut timeline) = Timeline::new(clips) {
            timeline.snapping = self.magnet;
            self.timeline = timeline;
            self.selected_keyframes = selected;
        }
    }

    fn apply_clip_trim(&mut self, pointer_x: f32) {
        let Some(drag) = self.clip_trim_drag.as_ref() else { return };
        let clip_id = drag.clip_id.clone();
        let linked_id = drag.linked_id.clone();
        let left_edge = drag.left_edge;
        let delta = (pointer_x - drag.origin_x) as f64 / drag.pixels_per_second.max(0.001) as f64;
        let original_start = drag.start;
        let original_duration = drag.duration;
        let original_offset = drag.source_offset;
        let original_end = original_start + original_duration;
        let minimum = 1.0 / self.fps.max(1.0);
        let source_duration = self.timeline.clips().iter().find(|clip| clip.id == clip_id)
            .and_then(|clip| clip.extra.get("sourceDuration")).and_then(serde_json::Value::as_f64);
        let mut new_start = if left_edge { (original_start + delta).clamp(0.0, original_end - minimum) } else { original_start };
        let mut new_duration = if left_edge { original_end - new_start } else { (original_duration + delta).max(minimum) };
        if left_edge {
            let candidate_offset = (original_offset + new_start - original_start).max(0.0);
            if let Some(source_duration) = source_duration {
                new_duration = new_duration.min((source_duration - candidate_offset).max(minimum));
                new_start = original_end - new_duration;
            }
        } else if let Some(source_duration) = source_duration {
            new_duration = new_duration.min((source_duration - original_offset).max(minimum));
        }
        let new_offset = if left_edge { (original_offset + new_start - original_start).max(0.0) } else { original_offset };
        let mut clips = self.timeline.clips().to_vec();
        for clip in &mut clips {
            if clip.id == clip_id || linked_id.as_deref() == Some(clip.id.as_str()) {
                clip.start = new_start;
                clip.duration = new_duration;
                clip.extra.insert("sourceOffset".to_owned(), serde_json::json!(new_offset));
            }
        }
        if let Ok(mut timeline) = Timeline::new(clips) {
            timeline.snapping = self.magnet;
            self.timeline = timeline;
            self.refresh_timeline_duration();
        }
    }

    fn apply_clip_group_drag(&mut self, pointer_x: f32, requested_track: TrackId) {
        let Some(drag) = self.clip_drag_origin.as_ref() else { return };
        let primary_id = drag.primary_id.clone();
        let origin_x = drag.origin_x;
        let pps = drag.pixels_per_second.max(0.001);
        let primary_start = drag.primary_start;
        let primary_track = drag.primary_track;
        let moving = drag.clips.clone();
        let moving_ids: HashSet<String> = moving.iter().map(|clip| clip.id.clone()).collect();
        let target_track = if requested_track.is_video() == primary_track.is_video() { requested_track } else { primary_track };
        let raw_delta = (pointer_x - origin_x) as f64 / pps as f64;
        let mut new_primary_start = (primary_start + raw_delta).max(0.0);
        if self.magnet {
            let threshold = 8.0 / pps as f64;
            let primary_duration = moving.iter().find(|clip| clip.id == primary_id).map(|clip| clip.duration).unwrap_or(0.0);
            let mut best: Option<(f64, f64)> = None;
            let candidates = std::iter::once(self.playhead).chain(self.timeline.clips().iter()
                .filter(|clip| !moving_ids.contains(&clip.id))
                .flat_map(|clip| [clip.start, clip.end()]));
            for boundary in candidates {
                for candidate in [boundary, boundary - primary_duration] {
                    let distance = (candidate - new_primary_start).abs();
                    if distance <= threshold && best.is_none_or(|(_, best_distance)| distance < best_distance) {
                        best = Some((candidate.max(0.0), distance));
                    }
                }
            }
            if let Some((snapped, _)) = best { new_primary_start = snapped; }
        }
        let delta = new_primary_start - primary_start;
        let lane_delta = track_lane_index(target_track) - track_lane_index(primary_track);
        let mut clips = self.timeline.clips().to_vec();
        for clip in &mut clips {
            let Some(original) = moving.iter().find(|original| original.id == clip.id) else { continue };
            clip.start = (original.start + delta).max(0.0);
            if original.id == primary_id {
                clip.track = target_track;
            } else if original.track.is_video() == primary_track.is_video() {
                clip.track = track_for_lane(original.track.is_video(), track_lane_index(original.track) + lane_delta);
            }
        }
        if let Ok(mut timeline) = Timeline::new(clips) {
            timeline.snapping = self.magnet;
            self.timeline = timeline;
            self.refresh_timeline_duration();
        }
    }

    fn push_undo(&mut self) {
        self.undo.push(self.timeline.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn undo_timeline(&mut self) {
        if let Some(previous) = self.undo.pop() {
            self.redo.push(self.timeline.clone());
            self.timeline = previous;
            self.refresh_timeline_duration();
        }
    }

    fn redo_timeline(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo.push(self.timeline.clone());
            self.timeline = next;
            self.refresh_timeline_duration();
        }
    }

    fn import_media(&mut self) {
        let files = rfd::FileDialog::new()
            .add_filter(
                "Media",
                &[
                    "mp4", "mov", "mkv", "avi", "webm", "m4v", "mp3", "wav", "m4a",
                    "aac", "flac", "ogg", "png", "jpg", "jpeg", "webp", "gif",
                ],
            )
            .pick_files();
        let Some(files) = files else { return };
        self.import_media_paths(files);
    }

    fn import_media_paths(&mut self, files: Vec<PathBuf>) {
        self.status = None;
        for path in files {
            let path = match path.canonicalize() {
                Ok(path) => path,
                Err(_) => path,
            };
            if self.pending_media_paths.contains(&path)
                || self.media.iter().any(|item| item.path == path)
            {
                continue;
            }
            let id = self.next_media_id;
            self.next_media_id += 1;
            self.pending_media_paths.insert(path.clone());
            if self.import_commands.send((id, path.clone())).is_err() {
                self.pending_media_paths.remove(&path);
                self.status = Some("Media import worker stopped unexpectedly".to_owned());
            }
        }
    }

    fn save_project(&mut self) {
        let dialog = rfd::FileDialog::new()
            .add_filter("Pluto project", &["pluto"])
            .set_file_name(format!("{}.pluto", safe_filename(&self.name)));
        if let Some(path) = dialog.save_file() {
            let project = serde_json::json!({
                "format": "pluto-project",
                "version": 1,
                "name": &self.name,
                "fps": self.fps,
                "timeline": &self.timeline,
            });
            match serde_json::to_vec_pretty(&project).and_then(|bytes| std::fs::write(&path, bytes).map_err(Into::into)) {
                Ok(()) => self.status = Some(format!("Saved project: {}", path.file_name().and_then(|name| name.to_str()).unwrap_or("project.pluto"))),
                Err(error) => self.status = Some(format!("Could not save project: {error}")),
            }
        }
    }

    fn open_project(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Pluto project", &["pluto", "json"])
            .pick_file() else { return };
        let loaded = std::fs::read(&path)
            .map_err(|error| error.to_string())
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|error| error.to_string()))
            .and_then(|value| {
                if value.get("format").and_then(serde_json::Value::as_str) != Some("pluto-project")
                    || value.get("version").and_then(serde_json::Value::as_u64) != Some(1) {
                    return Err("Unsupported Pluto project format or version".to_owned());
                }
                let name = value.get("name").and_then(serde_json::Value::as_str).unwrap_or("Untitled project").to_owned();
                let fps = value.get("fps").and_then(serde_json::Value::as_f64).filter(|fps| fps.is_finite() && *fps > 0.0).unwrap_or(30.0);
                let timeline: Timeline = serde_json::from_value(value.get("timeline").cloned().ok_or("Project has no timeline")?)
                    .map_err(|error| error.to_string())?;
                Ok((name, fps, timeline))
            });
        match loaded {
            Ok((name, fps, timeline)) => {
                let paths: HashSet<PathBuf> = timeline.clips().iter()
                    .filter_map(|clip| clip.extra.get("path").and_then(serde_json::Value::as_str).map(PathBuf::from))
                    .filter(|path| path.exists()).collect();
                self.playing = false;
                self.name = name;
                self.fps = fps;
                self.timeline = timeline;
                self.magnet = self.timeline.snapping;
                self.undo.clear();
                self.redo.clear();
                self.selected_clip = None;
                self.selected_clips.clear();
                self.selected_keyframes.clear();
                self.playhead = 0.0;
                self.preview_request = None;
                self.preview_texture = None;
                self.next_clip_id = self.timeline.clips().iter().filter_map(|clip| {
                    clip.id.rsplit('-').next().and_then(|value| value.parse::<u64>().ok())
                }).max().unwrap_or(0) + 1;
                self.next_group_id = self.timeline.clips().iter().filter_map(|clip| {
                    clip.extra.get("groupId").and_then(serde_json::Value::as_str)
                        .and_then(|value| value.strip_prefix("group-")).and_then(|value| value.parse::<u64>().ok())
                }).max().unwrap_or(0) + 1;
                self.refresh_timeline_duration();
                self.import_media_paths(paths.into_iter().collect());
                self.status = Some("Project opened".to_owned());
            }
            Err(error) => self.status = Some(format!("Could not open project: {error}")),
        }
    }

    fn poll_import_events(&mut self, ctx: &egui::Context) {
        while let Ok((path, result)) = self.import_events.try_recv() {
            self.pending_media_paths.remove(&path);
            match result {
                Ok(raw) => {
                    let thumbnail = raw.thumbnail.map(|(width, height, rgba)| {
                            let size = [width, height];
                            let color = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
                            ctx.load_texture(format!("media-thumb-{}", raw.id), color, egui::TextureOptions::LINEAR)
                        });
                    self.media.push(MediaItem {
                        id: raw.id,
                        path: raw.path,
                        name: raw.name,
                        editing_name: false,
                        duration: raw.duration,
                        kind: raw.kind,
                        is_still: raw.is_still,
                        has_audio: raw.has_audio,
                        thumbnail,
                        waveform: raw.waveform,
                    });
                    self.media.sort_by_key(|item| item.id);
                    self.status = None;
                }
                Err(error) => self.status = Some(error),
            }
        }
    }

    fn add_media_clip(&mut self, media_id: u64, start: f64, track: TrackId) {
        // Copy out the media metadata first so the immutable borrow of
        // `self.media` ends before `push_undo()` takes `&mut self`.
        let Some((item_path, item_name, item_duration, item_kind, item_is_still, item_has_audio, item_waveform)) = self
            .media
            .iter()
            .find(|item| item.id == media_id)
            .map(|item| (item.path.clone(), item.name.clone(), item.duration, item.kind, item.is_still, item.has_audio, item.waveform.clone()))
        else { return };
        if (item_kind == Kind::Video) != track.is_video() {
            return;
        }
        self.push_undo();
        let id = format!("clip-{}", self.next_clip_id);
        self.next_clip_id += 1;
        let mut extra = HashMap::new();
        extra.insert("path".to_owned(), serde_json::json!(item_path.to_string_lossy()));
        extra.insert("kind".to_owned(), serde_json::json!(if item_kind == Kind::Video { "video" } else { "audio" }));
        extra.insert("mediaKind".to_owned(), serde_json::json!(if item_is_still { "image" } else if item_kind == Kind::Video { "video" } else { "audio" }));
        extra.insert("hasAudio".to_owned(), serde_json::json!(item_has_audio));
        extra.insert("waveform".to_owned(), serde_json::json!(item_waveform));
        extra.insert("sourceDuration".to_owned(), serde_json::json!(item_duration));
        extra.insert("props".to_owned(), serde_json::json!({
            "transform": {
                "scale": { "w": 100.0, "h": 100.0, "linked": true },
                "position": { "x": 0.0, "y": 0.0 },
                "rotation": 0.0,
                "opacity": 100.0,
                "crop": { "top": 0.0, "right": 0.0, "bottom": 0.0, "left": 0.0 },
                "distort": { "tl": 0.0, "tr": 0.0, "br": 0.0, "bl": 0.0 }
            },
            "colors": {
                "light": { "exposure": 0.0, "brightness": 0.0, "contrast": 0.0 },
                "color": { "saturation": 0.0, "vibrance": 0.0, "temperature": 0.0 },
                "detail": { "clarity": 0.0, "sharpness": 0.0, "vignette": 0.0 },
                "grading": {}
            },
            "effects": {
                "preset": { "id": "none", "intensity": 100.0 },
                "blur": { "enabled": false, "mode": "gaussian", "amount": 25.0, "angle": 0.0 },
                "grain": { "enabled": false, "amount": 35.0, "size": 30.0, "roughness": 50.0 },
                "chromatic": { "enabled": false, "amount": 30.0, "angle": 0.0 },
                "glow": { "enabled": false, "amount": 30.0, "threshold": 70.0, "radius": 40.0 },
                "glitch": { "enabled": false, "amount": 30.0, "blocks": 25.0, "speed": 50.0 },
                "pixelate": { "enabled": false, "size": 12.0, "blend": 100.0 },
                "sharpen": { "enabled": false, "amount": 35.0, "radius": 5.0, "threshold": 10.0 },
                "lens": { "enabled": false, "amount": 0.0, "scale": 100.0, "blend": 100.0 },
                "fisheye": { "enabled": false, "amount": 35.0, "scale": 100.0, "blend": 100.0 },
                "mirror": { "enabled": false, "axis": "horizontal", "blend": 100.0 }
            },
            "audio": { "volume": 100.0, "pan": 0.0, "bass": 0.0, "treble": 0.0, "fadeIn": false, "fadeOut": false, "fadeInDuration": 1.0, "fadeOutDuration": 1.0 }
        }));
        let clip = Clip {
            id: id.clone(),
            track,
            start: start.max(0.0),
            duration: item_duration.max(0.1),
            label: item_name.clone(),
            linked_clip_id: None,
            extra,
        };
        let lanes: &[TrackId] = if track.is_video() {
            &[TrackId::V1, TrackId::V2, TrackId::V3, TrackId::V4]
        } else {
            &[TrackId::A1, TrackId::A2]
        };
        let mut failure = None;
        for lane in lanes {
            let mut candidate = clip.clone();
            candidate.track = *lane;
            let mut timeline = self.timeline.clone();
            timeline.snapping = self.magnet;
            match timeline.add_clip(candidate) {
                Ok(()) => {
                    self.timeline = timeline;
                    self.selected_clip = Some(id);
                    self.selected_clips.clear();
                    self.selected_clips.insert(self.selected_clip.clone().unwrap_or_default());
                    self.refresh_timeline_duration();
                    self.status = None;
                    return;
                }
                Err(error) => failure = Some(error),
            }
        }
        self.status = failure.map(|error| format!("Could not add clip: {error:?}"));
    }

    fn finish_media_drop(&mut self, ctx: &egui::Context) {
        let Some(media_id) = self.dragging_media else { return };
        if !ctx.input(|input| input.pointer.any_released()) {
            return;
        }
        self.dragging_media = None;
        let Some(pointer) = ctx.input(|input| input.pointer.interact_pos()) else { return };
        let Some((track, row)) = self.track_hit_areas.iter().find(|(_, row)| row.contains(pointer)) else { return };
        if !self.timeline_content.contains(pointer) || self.timeline_content.width() <= 0.0 {
            return;
        }
        let track = *track;
        let pps = self.timeline_content.width() / self.duration.max(1.0) as f32 * self.zoom;
        let start = self.timeline_offset + ((pointer.x - self.timeline_content.left()) / pps.max(0.001)) as f64;
        let _ = row;
        self.add_media_clip(media_id, start, track);
    }
}

impl eframe::App for PlutoApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // eframe initially hides the native window to avoid a blank flash.
        // Ask it to render once while hidden so it can paint the editor and
        // reveal the window on the first frame.
        if !self.startup_paint_requested {
            self.startup_paint_requested = true;
            crate::log_startup_event("Requesting the first egui paint while the native window is hidden");
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        // egui 0.37 gives the application its root Ui directly. The root Ui
        // replaces the old Context -> CentralPanel::show entry point.
        let first_frame = !self.startup_frame_started;
        if first_frame {
            self.startup_frame_started = true;
            crate::log_startup_event("First editor UI frame started");
        }
        let ctx = ui.ctx().clone();
        self.tick(&ctx);
        if first_frame {
            crate::log_startup_event("Initial editor state update completed");
        }
        self.refresh_preview(&ctx);
        if first_frame {
            crate::log_startup_event("Initial preview update completed");
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(theme::BG))
            .show(ui, |ui| self.shell(ui));
        if first_frame {
            crate::log_startup_event("Initial editor panels painted");
        }
        if self.show_export_settings && !self.export_busy {
            self.export_settings_window(&ctx);
        }
        if first_frame && !self.startup_frame_finished {
            self.startup_frame_finished = true;
            crate::log_startup_event("First editor UI frame completed");
        }
    }
}

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

impl PlutoApp {
    fn refresh_preview(&mut self, ctx: &egui::Context) {
        while let Ok(frame) = self.preview_frames.try_recv() {
            if frame.request_id == self.preview_request_id {
                let image = egui::ColorImage::from_rgba_unmultiplied([PREVIEW_WIDTH, PREVIEW_HEIGHT], &frame.rgba);
                if let Some(texture) = self.preview_texture.as_mut() {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    self.preview_texture = Some(ctx.load_texture("pluto-live-preview", image, egui::TextureOptions::LINEAR));
                }
            }
        }

        let active = self.timeline.active_clips_at(self.playhead, None).into_iter()
            .find(|clip| clip.track.is_video() && clip.extra.get("path").and_then(serde_json::Value::as_str).is_some())
            .cloned();
        let Some(clip) = active else {
            if self.preview_request.as_ref().map(|request| request.0.as_str()) != Some("") {
                self.preview_request_id = self.preview_request_id.wrapping_add(1);
                let _ = self.preview_commands.send(PreviewCommand::Stop);
                self.preview_request = Some((String::new(), false, -2, 0));
                self.preview_texture = None;
            }
            return;
        };
        let Some(path_text) = clip.extra.get("path").and_then(|value| value.as_str()).map(str::to_owned) else { return };
        let Some((media_path, is_still)) = self.media.iter()
            .find(|item| item.path.to_string_lossy() == path_text)
            .map(|item| (item.path.clone(), item.is_still)) else { return };
        let frame_slot = if self.playing { -1 } else { (self.playhead * self.fps.max(1.0)).round() as i64 };
        let source_time = if is_still { 0.0 } else {
            (self.playhead - clip.start + clip.extra.get("sourceOffset").and_then(|value| value.as_f64()).unwrap_or(0.0)).max(0.0)
        };
        let mut clip_value = serde_json::to_value(&clip).unwrap_or(serde_json::Value::Null);
        clip_value["sourceOffset"] = serde_json::json!(0.0);
        let filter_graph = pluto_media_engine::preview_video_graph(&clip_value, PREVIEW_WIDTH as u32, PREVIEW_HEIGHT as u32, self.playhead);
        let fingerprint_graph = if self.playing {
            // Keep the playback identity independent of the moving playhead;
            // the active FFmpeg process already advances frames continuously.
            pluto_media_engine::preview_video_graph(&clip_value, PREVIEW_WIDTH as u32, PREVIEW_HEIGHT as u32, clip.start)
        } else {
            filter_graph.clone()
        };
        let mut filter_hash = std::collections::hash_map::DefaultHasher::new();
        fingerprint_graph.hash(&mut filter_hash);
        clip.extra.get("sourceOffset").and_then(serde_json::Value::as_f64).unwrap_or(0.0).to_bits().hash(&mut filter_hash);
        if !self.playing {
            source_time.to_bits().hash(&mut filter_hash);
        }
        let desired = (path_text, self.playing, frame_slot, filter_hash.finish());
        if self.preview_request.as_ref() == Some(&desired) { return; }
        let editing_while_playing = self.playing
            && self.last_edit_field.is_some()
            && ctx.input(|input| input.pointer.any_down())
            && self.preview_request.as_ref().is_some_and(|previous| previous.0 == desired.0 && previous.1 && previous.3 != desired.3);
        if editing_while_playing { return; }
        self.preview_request = Some(desired);
        self.preview_request_id = self.preview_request_id.wrapping_add(1);
        let request_id = self.preview_request_id;
        let _ = self.preview_commands.send(PreviewCommand::Decode {
            request_id,
            path: media_path,
            source_time,
            continuous: self.playing,
            loop_image: is_still,
            filter_graph,
        });
    }

    fn create_text_clip(&mut self, box_rect: Rect) {
        if self.stage_rect.width() <= 0.0 || self.stage_rect.height() <= 0.0 { return; }
        self.push_undo();
        let id = format!("title-{}", self.next_clip_id);
        self.next_clip_id += 1;
        let x = (box_rect.center().x - self.stage_rect.center().x) as f64 * 1920.0 / self.stage_rect.width() as f64;
        let y = (box_rect.center().y - self.stage_rect.center().y) as f64 * 1080.0 / self.stage_rect.height() as f64;
        let size = (box_rect.height() as f64 * 1080.0 / self.stage_rect.height() as f64 / 48.0 * 100.0).clamp(15.0, 400.0);
        let mut extra = HashMap::new();
        extra.insert("theme".to_owned(), serde_json::json!("title"));
        extra.insert("props".to_owned(), serde_json::json!({
            "text": { "title": "Title", "sub": "", "caption": "", "size": size, "align": "center" },
            "transform": {
                "scale": { "w": 100.0, "h": 100.0, "linked": true },
                "position": { "x": x, "y": y }, "rotation": 0.0, "opacity": 100.0,
                "crop": { "top": 0.0, "right": 0.0, "bottom": 0.0, "left": 0.0 }
            }
        }));
        let clip = Clip {
            id: id.clone(),
            track: TrackId::V4,
            start: self.playhead.max(0.0),
            duration: 5.0,
            label: "Title".to_owned(),
            linked_clip_id: None,
            extra,
        };
        if self.timeline.add_clip(clip).is_ok() {
            self.selected_clip = Some(id);
            self.selected_clips.clear();
            if let Some(id) = self.selected_clip.clone() { self.selected_clips.insert(id); }
            self.inspector_tab = InspectorTab::Text;
            self.refresh_timeline_duration();
        }
    }

    fn shell(&mut self, ui: &mut Ui) {
        let full = ui.max_rect();
        let painter = ui.painter().clone();

        painter.rect_filled(full, CornerRadius::ZERO, theme::BG);

        // Row 1 — the top bar spans the full width (`margin: 0 -10px`).
        let top = Rect::from_min_size(full.min, Vec2::new(full.width(), theme::ROW_TOP));
        self.top_bar(ui, top);

        // Rows 2/3 live inside the 10px horizontal / bottom padding.
        let x0 = full.min.x + theme::PAD_X;
        let x1 = full.max.x - theme::PAD_X;
        let y0 = full.min.y + theme::ROW_TOP + theme::GAP;
        let y_bottom = full.max.y - theme::PAD_BOTTOM;

        let tl_h = theme::ROW_TIMELINE
            .min(((y_bottom - y0).max(0.0)) * 0.6)
            .max(0.0);
        let mid_h = (y_bottom - y0 - theme::GAP - tl_h).max(0.0);

        let narrow = full.width() <= theme::MEDIA_BREAKPOINT;
        let media_w = if narrow { 0.0 } else { theme::COL_MEDIA };
        let inspector_w = if narrow { 400.0 } else { theme::COL_INSPECTOR };

        let mut left = x0;
        if !narrow {
            // `grid-area: media` — row 2 only.
            let media = Rect::from_min_size(Pos2::new(left, y0), Vec2::new(media_w, mid_h));
            self.media_column(ui, media);
            left += media_w + theme::GAP;
        }

        let inspector_x = x1 - inspector_w;
        let preview = Rect::from_min_size(
            Pos2::new(left, y0),
            Vec2::new((inspector_x - theme::GAP - left).max(0.0), mid_h),
        );
        self.preview(ui, preview);

        let inspector = Rect::from_min_size(
            Pos2::new(inspector_x, y0),
            Vec2::new(inspector_w, mid_h),
        );
        self.inspector(ui, inspector);

        let timeline = Rect::from_min_size(
            Pos2::new(x0, y0 + mid_h + theme::GAP),
            Vec2::new(x1 - x0, tl_h),
        );
        self.timeline(ui, timeline);
    }

    // -- top bar ------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter().clone();
        painter.rect_filled(rect, CornerRadius::ZERO, theme::TOP_BG);
        theme::hairline(
            &painter,
            Rect::from_min_size(Pos2::new(rect.min.x, rect.max.y - 1.0), Vec2::new(rect.width(), 1.0)),
            theme::BORDER,
        );

        let mid_y = rect.center().y;

        // `.ed-logo` — 30×30, radius 8.
        let logo_rect = Rect::from_center_size(
            Pos2::new(rect.min.x + 14.0 + 15.0, mid_y),
            Vec2::splat(30.0),
        );
        match &self.logo {
            Some(tex) => {
                painter.image(
                    tex.id(),
                    logo_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                painter.rect_filled(logo_rect, CornerRadius::same(8), theme::BLUE);
            }
        }

        // `.ed-brand`
        let brand = painter.text(
            Pos2::new(logo_rect.max.x + 12.0, mid_y),
            Align2::LEFT_CENTER,
            "Pluto",
            theme::bold(theme::FONT_BRAND),
            theme::TEXT,
        );

        // WINDOW CONTROLS — `.ed-window-controls { margin-right: -16px }`
        let mut right = rect.max.x;
        let wc_w = 44.0 * 3.0;
        let controls = Rect::from_min_max(
            Pos2::new(right - wc_w, rect.min.y),
            Pos2::new(right, rect.max.y),
        );
        right -= wc_w;

        let close = self.window_button(ui, &painter, controls, 2, "x", true);
        if close.clicked() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let maximize = self.window_button(ui, &painter, controls, 1, "windowMaximize", false);
        if maximize.clicked() {
            let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
        }
        let minimize = self.window_button(ui, &painter, controls, 0, "windowMinimize", false);
        if minimize.clicked() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }

        // `.ed-export`
        right -= 12.0;
        let export_w = 104.0;
        let export = Rect::from_center_size(
            Pos2::new(right - export_w * 0.5, mid_y),
            Vec2::new(export_w, 36.0),
        );
        right = export.min.x;

        let export_resp = ui.interact(export, egui::Id::new("ed-export"), Sense::click());
        painter.add(egui::Shape::mesh(theme::gradient_card(
            export,
            12.0,
            theme::EXPORT_TOP,
            theme::EXPORT_BOTTOM,
        )));
        painter.rect_stroke(
            export,
            CornerRadius::same(12),
            Stroke::new(1.0, theme::HAIRLINE),
            StrokeKind::Inside,
        );
        if export_resp.hovered() {
            painter.rect_filled(
                export,
                CornerRadius::same(12),
                Color32::from_rgba_premultiplied(12, 12, 12, 12),
            );
        }
        icons::draw_at(
            &painter,
            Pos2::new(export.min.x + 14.0 + 8.0, mid_y),
            16.0,
            "upload",
            theme::TEXT,
        );
        painter.text(
            Pos2::new(export.min.x + 14.0 + 16.0 + 8.0, mid_y),
            Align2::LEFT_CENTER,
            if self.export_busy { "Cancel" } else { "Export" },
            theme::bold(14.0),
            theme::TEXT,
        );
        if self.export_busy {
            let progress = Rect::from_min_size(
                export.min + Vec2::new(8.0, export.height() - 3.0),
                Vec2::new((export.width() - 16.0) * self.export_progress as f32, 2.0),
            );
            painter.rect_filled(progress, CornerRadius::same(1), theme::BLUE);
        }
        if export_resp.clicked() {
            self.start_or_cancel_export();
        }

        // `.ed-top__actions` — 38×38 icon buttons, 3px gap.
        right -= 8.0;
        let actions = [
            ("undo", "Undo"),
            ("redo", "Redo"),
            ("keyAdd", "Add keyframe"),
            ("grid", "Guides"),
        ];
        for (idx, (icon, label)) in actions.iter().enumerate().rev() {
            let b = Rect::from_center_size(
                Pos2::new(right - 19.0, mid_y),
                Vec2::splat(38.0),
            );
            let resp = self.icon_button(ui, &painter, b, ("top-action", idx), *icon, 20.0, theme::MUTED);
            let resp = resp.on_hover_text(*label);
            if resp.clicked() {
                match idx {
                    0 => self.undo_timeline(),
                    1 => self.redo_timeline(),
                    2 => self.toggle_keyframe(),
                    3 => self.show_guides = !self.show_guides,
                    _ => {}
                }
            }
            right = b.min.x - 3.0;
        }

        // `.ed-name` — 220×38
        let name_w = 220.0;
        let name_rect = Rect::from_center_size(
            Pos2::new(right - name_w * 0.5, mid_y),
            Vec2::new(name_w, 38.0),
        );
        let name_resp = ui.interact(name_rect, egui::Id::new("ed-name"), Sense::click());
        if name_resp.hovered() {
            theme::field(&painter, name_rect, 12, theme::FIELD);
        }
        if name_resp.double_clicked() { self.editing_name = true; }
        if self.editing_name {
            let response = ui.put(name_rect.shrink2(Vec2::new(8.0, 3.0)), egui::TextEdit::singleline(&mut self.name).frame(egui::Frame::NONE));
            if name_resp.double_clicked() { response.request_focus(); }
            if response.lost_focus() && ui.ctx().input(|input| input.key_pressed(egui::Key::Enter)) { self.editing_name = false; }
        } else {
            painter.text(
                Pos2::new(name_rect.min.x + 12.0, mid_y),
                Align2::LEFT_CENTER,
                &self.name,
                theme::font(theme::FONT_BASE),
                theme::TEXT,
            );
        }
        right = name_rect.min.x;

        // `.ed-top__drag` — everything between the brand and the name field
        // drags the window.
        let drag = Rect::from_min_max(
            Pos2::new(brand.max.x + 12.0, rect.min.y),
            Pos2::new((right - 12.0).max(brand.max.x + 36.0), rect.max.y),
        );
        let drag_resp = ui.interact(drag, egui::Id::new("ed-drag"), Sense::drag());
        if drag_resp.drag_started() {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag_resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    fn window_button(
        &self,
        ui: &mut Ui,
        painter: &egui::Painter,
        controls: Rect,
        index: usize,
        icon: &str,
        close: bool,
    ) -> Response {
        let w = 44.0;
        let x = controls.max.x - w * (index as f32 + 1.0);
        let rect = Rect::from_min_size(Pos2::new(x, controls.min.y), Vec2::new(w, controls.height()));
        let resp = ui.interact(
            rect,
            egui::Id::new(("ed-wc", index)),
            Sense::click(),
        );
        if resp.hovered() {
            painter.rect_filled(
                rect,
                CornerRadius::ZERO,
                if close { theme::CLOSE_HOVER } else { theme::HOVER_STRONG },
            );
        }
        let color = if resp.hovered() && close { Color32::WHITE } else if resp.hovered() { theme::TEXT } else { theme::MUTED };
        icons::draw_at(painter, rect.center(), 16.0, icon, color);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    fn icon_button(
        &self,
        ui: &mut Ui,
        painter: &egui::Painter,
        rect: Rect,
        id: impl std::hash::Hash + std::fmt::Debug,
        icon: &str,
        icon_size: f32,
        tint: Color32,
    ) -> Response {
        let resp = ui.interact(rect, egui::Id::new(id), Sense::click());
        let hovered = resp.hovered();
        if hovered {
            painter.rect_filled(rect, CornerRadius::same(13), theme::HOVER);
        }
        icons::draw_at(
            painter,
            rect.center(),
            icon_size,
            icon,
            if hovered { theme::TEXT } else { tint },
        );
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }

    // -- media column -------------------------------------------------------

    fn media_column(&mut self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter().clone();
        theme::card(&painter, rect);

        let inner = rect.shrink2(Vec2::new(14.0, 14.0));
        let head_y = inner.min.y + 10.0;
        icons::draw_at(
            &painter,
            Pos2::new(inner.min.x + 9.0, head_y + 10.0),
            20.0,
            "film",
            theme::MUTED,
        );
        painter.text(
            Pos2::new(inner.min.x + 34.0, head_y + 10.0),
            Align2::LEFT_CENTER,
            "Library",
            theme::bold(theme::FONT_CARD_HEAD),
            theme::TEXT,
        );

        let import = Rect::from_min_size(
            Pos2::new(inner.max.x - 82.0, head_y - 6.0),
            Vec2::new(82.0, 32.0),
        );
        theme::field(&painter, import, 12, theme::FIELD);
        icons::draw_at(&painter, Pos2::new(import.min.x + 15.0, import.center().y), 14.0, "plus", theme::TEXT);
        painter.text(
            Pos2::new(import.min.x + 27.0, import.center().y),
            Align2::LEFT_CENTER,
            "Import",
            theme::font(theme::FONT_SMALL),
            theme::TEXT,
        );
        if ui.interact(import, egui::Id::new("media-import"), Sense::click()).clicked() {
            self.import_media();
        }

        let tabs = Rect::from_min_size(
            Pos2::new(inner.min.x, head_y + 30.0),
            Vec2::new(inner.width(), 40.0),
        );
        painter.rect_filled(tabs, CornerRadius::same(14), theme::TRACK);
        let tab_w = (tabs.width() - 6.0) * 0.5;
        for (index, (is_audio, label)) in [(false, "Media"), (true, "Audio")].into_iter().enumerate() {
            let tab = Rect::from_min_size(
                Pos2::new(tabs.min.x + 3.0 + index as f32 * tab_w, tabs.min.y + 3.0),
                Vec2::new(tab_w, tabs.height() - 6.0),
            );
            if self.show_audio_library == is_audio {
                painter.rect_filled(tab, CornerRadius::same(12), theme::TOOL_ACTIVE);
            }
            painter.text(tab.center(), Align2::CENTER_CENTER, label, theme::font(theme::FONT_SMALL), if self.show_audio_library == is_audio { theme::TEXT } else { theme::MUTED });
            if ui.interact(tab, egui::Id::new(("library-tab", index)), Sense::click()).clicked() {
                self.show_audio_library = is_audio;
            }
        }

        // `.ed-searchwrap` — 40px tall, radius 13.
        let search = Rect::from_min_size(
            Pos2::new(inner.min.x, tabs.max.y + 10.0),
            Vec2::new(inner.width(), 38.0),
        );
        theme::field(&painter, search, 13, theme::FIELD);
        icons::draw_at(
            &painter,
            Pos2::new(search.min.x + 12.0 + 8.0, search.center().y),
            16.0,
            "search",
            theme::MUTED,
        );
        ui.put(
            Rect::from_min_max(Pos2::new(search.min.x + 34.0, search.min.y + 2.0), search.max - Vec2::new(8.0, 2.0)),
            egui::TextEdit::singleline(&mut self.media_search)
                .hint_text("Search media")
                .frame(egui::Frame::NONE),
        );

        let list = Rect::from_min_max(Pos2::new(inner.min.x, search.max.y + 10.0), inner.max);
        if list.contains(ui.ctx().input(|i| i.pointer.hover_pos().unwrap_or(Pos2::new(-1.0, -1.0)))) {
            let scroll = ui.ctx().input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.media_scroll = (self.media_scroll - scroll).max(0.0);
            }
        }
        let clip = ui.painter().with_clip_rect(list);
        let filter = self.media_search.to_lowercase();
        let indices: Vec<usize> = self.media.iter().enumerate()
            .filter(|(_, item)| item.name.to_lowercase().contains(&filter))
            .filter(|(_, item)| !self.show_audio_library || item.kind == Kind::Audio)
            .map(|(i, _)| i).collect();
        let gap = 8.0;
        let cell_w = (list.width() - gap) * 0.5;
        let cell_h = 112.0;
        let rows = indices.len().div_ceil(2);
        let max_scroll = (rows as f32 * (cell_h + gap) - list.height()).max(0.0);
        self.media_scroll = self.media_scroll.min(max_scroll);
        for (slot, index) in indices.iter().copied().enumerate() {
            let item = &self.media[index];
            let col = slot % 2;
            let row = slot / 2;
            let cell = Rect::from_min_size(
                Pos2::new(list.min.x + col as f32 * (cell_w + gap), list.min.y + row as f32 * (cell_h + gap) - self.media_scroll),
                Vec2::new(cell_w, cell_h),
            );
            if !cell.intersects(list) { continue; }
            let thumb = Rect::from_min_size(cell.min, Vec2::new(cell.width(), 72.0));
            clip.rect_filled(cell, CornerRadius::same(12), theme::FIELD);
            if let Some(texture) = &item.thumbnail {
                clip.image(texture.id(), thumb.shrink(2.0), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            } else {
                clip.rect_filled(thumb.shrink(2.0), CornerRadius::same(10), theme::CARD);
                icons::draw_at(&clip, thumb.center(), 22.0, if item.kind == Kind::Audio { "music" } else { "video" }, theme::MUTED);
            }
            let duration = format_seconds(item.duration);
            let duration_rect = Rect::from_min_size(Pos2::new(thumb.min.x + 5.0, thumb.max.y - 19.0), Vec2::new(38.0, 16.0));
            clip.rect_filled(duration_rect, CornerRadius::same(6), theme::TRACK);
            clip.text(duration_rect.center(), Align2::CENTER_CENTER, duration, theme::bold(10.0), theme::TEXT);
            let name = truncate(&item.name, 22);
            clip.text(Pos2::new(cell.min.x + 2.0, thumb.max.y + 7.0), Align2::LEFT_TOP, name, theme::font(theme::FONT_TINY), theme::MUTED);
            let response = ui.interact(cell, egui::Id::new(("media-item", item.id)), Sense::click_and_drag());
            if response.drag_started() { self.dragging_media = Some(item.id); }
            if response.clicked() {
                let track = if item.kind == Kind::Audio { TrackId::A1 } else { TrackId::V1 };
                self.add_media_clip(item.id, self.playhead, track);
            }
        }
        if indices.is_empty() {
            let empty_y = list.min.y + 32.0;
            icons::draw_at(&clip, Pos2::new(list.center().x, empty_y), 24.0, "upload", theme::MUTED);
            clip.text(Pos2::new(list.center().x, empty_y + 26.0), Align2::CENTER_TOP, if self.media.is_empty() { "Import media to get started" } else { "No matching media" }, theme::font(theme::FONT_SMALL), theme::MUTED);
        }
        if let Some(error) = &self.status {
            let status_rect = Rect::from_min_size(Pos2::new(list.min.x, list.max.y - 32.0), Vec2::new(list.width(), 28.0));
            clip.text(status_rect.min, Align2::LEFT_TOP, truncate(error, 34), theme::font(theme::FONT_TINY), theme::RED);
        }
    }

    // -- preview ------------------------------------------------------------

    fn preview(&mut self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter().clone();
        theme::card(&painter, rect);

        // `.ed-stage` — width: min(100cqw - 28, (100cqh - 104) * 16/9)
        let avail_w = (rect.width() - 28.0).max(0.0);
        let avail_h = (rect.height() - 104.0).max(0.0);
        let stage_w = avail_w.min(avail_h * 16.0 / 9.0);
        let stage_h = stage_w * 9.0 / 16.0;
        let stage = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.min.y + (rect.height() - 104.0) * 0.5),
            Vec2::new(stage_w, stage_h),
        );
        self.stage_rect = stage;

        // `.ed-frame`
        painter.rect_filled(stage, CornerRadius::same(12), Color32::BLACK);
        painter.rect_stroke(
            stage,
            CornerRadius::same(12),
            Stroke::new(1.0, theme::HAIRLINE),
            StrokeKind::Inside,
        );
        if let Some(texture) = self.preview_texture.as_ref().or_else(|| self.preview_thumbnail()) {
            let size = texture.size_vec2();
            let scale = (stage.width() / size.x.max(1.0)).min(stage.height() / size.y.max(1.0));
            let fitted = size * scale;
            let image_rect = Rect::from_center_size(stage.center(), fitted);
            painter.with_clip_rect(stage).image(texture.id(), image_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        let title_clips: Vec<Clip> = self.timeline.active_clips_at(self.playhead, None).into_iter()
            .filter(|clip| clip.extra.get("theme").and_then(serde_json::Value::as_str) == Some("title"))
            .cloned().collect();
        for clip in title_clips {
            let text = clip.extra.get("props").and_then(|props| props.get("text"));
            let transform = clip.extra.get("props").and_then(|props| props.get("transform"));
            let title = text.and_then(|value| value.get("title")).and_then(serde_json::Value::as_str).unwrap_or("");
            let subtitle = text.and_then(|value| value.get("sub")).and_then(serde_json::Value::as_str).unwrap_or("");
            let size = text.and_then(|value| value.get("size")).and_then(serde_json::Value::as_f64).unwrap_or(100.0);
            let x = transform.and_then(|value| value.get("position")).and_then(|value| value.get("x")).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
            let y = transform.and_then(|value| value.get("position")).and_then(|value| value.get("y")).and_then(serde_json::Value::as_f64).unwrap_or(0.0);
            let opacity = transform.and_then(|value| value.get("opacity")).and_then(serde_json::Value::as_f64).unwrap_or(100.0);
            let font_size = (48.0 * size / 100.0 * stage.height() as f64 / 1080.0).clamp(8.0, 180.0) as f32;
            let center = Pos2::new(stage.center().x + (x * stage.width() as f64 / 1920.0) as f32, stage.center().y + (y * stage.height() as f64 / 1080.0) as f32);
            let color = Color32::from_white_alpha((opacity.clamp(0.0, 100.0) * 2.55) as u8);
            let align = text.and_then(|value| value.get("align")).and_then(serde_json::Value::as_str).unwrap_or("center");
            let text_align = match align { "left" => Align2::LEFT_CENTER, "right" => Align2::RIGHT_CENTER, _ => Align2::CENTER_CENTER };
            if !title.is_empty() {
                painter.text(center, text_align, title, theme::bold(font_size), color);
            }
            if !subtitle.is_empty() {
                painter.text(center + Vec2::new(0.0, font_size * 0.7), text_align, subtitle, theme::font(font_size * 0.42), color);
            }
            if self.selected_clip.as_deref() == Some(clip.id.as_str()) {
                let bounds = Rect::from_center_size(center, Vec2::new((font_size * title.chars().count().max(3) as f32 * 0.62).max(64.0), (font_size * 1.8).max(44.0)));
                painter.rect_stroke(bounds, CornerRadius::same(2), Stroke::new(1.0, theme::SELECT_RING), StrokeKind::Inside);
                if self.tool == Tool::Cursor {
                    let response = ui.interact(bounds, egui::Id::new(("preview-title", clip.id.as_str())), Sense::click_and_drag());
                    if response.drag_started() {
                        if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                            self.title_drag_origin = Some((clip.id.clone(), pointer, x, y));
                        }
                    }
                    if response.dragged() {
                        if let (Some((id, origin, start_x, start_y)), Some(pointer)) = (self.title_drag_origin.clone(), ui.ctx().pointer_interact_pos()) {
                            let position_x = start_x + (pointer.x - origin.x) as f64 * 1920.0 / stage.width() as f64;
                            let position_y = start_y + (pointer.y - origin.y) as f64 * 1080.0 / stage.height() as f64;
                            self.set_title_position(&id, position_x, position_y);
                        }
                    }
                    if response.drag_stopped() { self.title_drag_origin = None; }
                }
            }
        }
        let caption_clips: Vec<Clip> = self.timeline.active_clips_at(self.playhead, None).into_iter()
            .filter(|clip| clip.extra.get("props").and_then(|props| props.get("text"))
                .and_then(|text| text.get("caption")).and_then(serde_json::Value::as_str).is_some_and(|value| !value.is_empty()))
            .cloned().collect();
        for clip in caption_clips {
            let caption = clip.extra.get("props").and_then(|props| props.get("text"))
                .and_then(|text| text.get("caption")).and_then(serde_json::Value::as_str).unwrap_or("");
            let size = (32.0 * stage.height() / 1080.0).clamp(8.0, 72.0);
            painter.text(
                Pos2::new(stage.center().x, stage.max.y - 32.0 * stage.height() / 1080.0),
                Align2::CENTER_CENTER,
                caption,
                theme::bold(size),
                theme::TEXT,
            );
        }
        if self.tool == Tool::Text {
            let placement = ui.interact(stage, egui::Id::new("preview-text-placement"), Sense::click_and_drag());
            if placement.drag_started() {
                self.text_box_drag = ui.ctx().pointer_interact_pos().filter(|pos| stage.contains(*pos));
            }
            if let (Some(start), Some(current)) = (self.text_box_drag, ui.ctx().pointer_interact_pos()) {
                let box_rect = Rect::from_two_pos(start, current).intersect(stage);
                painter.rect_filled(box_rect, CornerRadius::same(3), Color32::from_rgba_premultiplied(59, 130, 246, 18));
                painter.rect_stroke(box_rect, CornerRadius::same(3), Stroke::new(1.0, theme::BLUE), StrokeKind::Inside);
            }
            if placement.drag_stopped() {
                if let (Some(start), Some(end)) = (self.text_box_drag.take(), ui.ctx().pointer_interact_pos()) {
                    let box_rect = Rect::from_two_pos(start, end).intersect(stage);
                    if box_rect.width() >= 8.0 && box_rect.height() >= 8.0 {
                        self.create_text_clip(box_rect);
                    }
                }
            }
        }
        if self.show_guides {
            let guide_color = Color32::from_rgba_premultiplied(150, 150, 150, 100);
            for fraction in [1.0 / 3.0, 2.0 / 3.0] {
                let x = stage.min.x + stage.width() * fraction;
                let y = stage.min.y + stage.height() * fraction;
                painter.line_segment([Pos2::new(x, stage.min.y), Pos2::new(x, stage.max.y)], Stroke::new(1.0, guide_color));
                painter.line_segment([Pos2::new(stage.min.x, y), Pos2::new(stage.max.x, y)], Stroke::new(1.0, guide_color));
            }
        }

        // `.ed-transport` — grid 1fr auto 1fr, padding 12 16 14.
        let bar_h = 56.0;
        let bar = Rect::from_min_size(
            Pos2::new(rect.min.x, rect.max.y - bar_h),
            Vec2::new(rect.width(), bar_h),
        );
        let mid_y = bar.center().y;

        // Left: current / total timecode.
        let tc = Rect::from_min_size(
            Pos2::new(bar.min.x + 16.0, mid_y - 11.0),
            Vec2::new(bar.width() / 3.0, 22.0),
        );
        let cur = painter.text(
            Pos2::new(tc.min.x, mid_y),
            Align2::LEFT_CENTER,
            timecode(self.playhead, self.fps),
            theme::bold(theme::FONT_TC),
            theme::TEXT,
        );
        let sep = painter.text(
            Pos2::new(cur.max.x + 8.0, mid_y),
            Align2::LEFT_CENTER,
            "/",
            theme::font(theme::FONT_TC),
            theme::MUTED,
        );
        painter.text(
            Pos2::new(sep.max.x + 8.0, mid_y),
            Align2::LEFT_CENTER,
            timecode(self.project_duration, self.fps),
            theme::font(theme::FONT_TC),
            theme::MUTED,
        );

        // Middle: skip back / play / skip forward.
        let cx = bar.center().x;
        let play_rect = Rect::from_center_size(Pos2::new(cx, mid_y), Vec2::splat(38.0));
        let back_rect = Rect::from_center_size(
            Pos2::new(play_rect.min.x - 6.0 - 19.0, mid_y),
            Vec2::splat(38.0),
        );
        let fwd_rect = Rect::from_center_size(
            Pos2::new(play_rect.max.x + 6.0 + 19.0, mid_y),
            Vec2::splat(38.0),
        );

        if self
            .icon_button(ui, &painter, back_rect, "tr-back", "skipBack", 20.0, theme::MUTED)
            .clicked()
        {
            self.playing = false;
            self.playhead = (self.playhead - 1.0 / self.fps.max(1.0)).max(0.0);
        }
        let play = self.icon_button(
            ui,
            &painter,
            play_rect,
            "tr-play",
            if self.playing { "pause" } else { "play" },
            20.0,
            theme::TEXT,
        );
        if play.clicked() {
            self.playing = !self.playing;
            if self.playing && self.playhead >= self.project_duration { self.playhead = 0.0; }
        }
        if self
            .icon_button(ui, &painter, fwd_rect, "tr-fwd", "skipForward", 20.0, theme::MUTED)
            .clicked()
        {
            self.playing = false;
            self.playhead = (self.playhead + 1.0 / self.fps.max(1.0)).min(self.project_duration);
        }

        // Right: zoom.
        let zoom_rect = Rect::from_center_size(
            Pos2::new(bar.max.x - 16.0 - 19.0, mid_y),
            Vec2::splat(38.0),
        );
        if self
            .icon_button(ui, &painter, zoom_rect, "tr-zoomin", "zoomIn", 20.0, theme::MUTED)
            .clicked()
        {
            self.zoom = (self.zoom * 1.25).min(8.0);
        }
        let zoom_out = Rect::from_center_size(
            Pos2::new(zoom_rect.min.x - 3.0 - 38.0 - 3.0 - 19.0, mid_y),
            Vec2::splat(38.0),
        );
        if self
            .icon_button(ui, &painter, zoom_out, "tr-zoomout", "zoomOut", 20.0, theme::MUTED)
            .clicked()
        {
            self.zoom = (self.zoom / 1.25).max(0.25);
        }
        painter.text(
            Pos2::new(zoom_rect.min.x - 3.0 - 38.0, mid_y),
            Align2::RIGHT_CENTER,
            format!("{:.0}%", self.zoom * 100.0),
            theme::font(theme::FONT_SMALL),
            theme::MUTED,
        );
    }

    fn preview_thumbnail(&self) -> Option<&egui::TextureHandle> {
        self.timeline.active_clips_at(self.playhead, None).into_iter()
            .find(|clip| clip.track.is_video())
            .and_then(|clip| clip.extra.get("path").and_then(|value| value.as_str()))
            .and_then(|path| self.media.iter().find(|item| item.path.to_string_lossy() == path))
            .and_then(|item| item.thumbnail.as_ref())
    }

    // -- inspector ----------------------------------------------------------

    fn inspector(&mut self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter().clone();
        theme::card(&painter, rect);

        let inner = rect.shrink2(Vec2::new(12.0, 12.0));
        let head = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 38.0));
        icons::draw_at(&painter, Pos2::new(head.min.x + 18.0, head.center().y), 22.0, "crop", theme::MUTED);
        painter.text(Pos2::new(head.min.x + 38.0, head.center().y), Align2::LEFT_CENTER, "Inspector", theme::bold(theme::FONT_CARD_HEAD), theme::TEXT);

        let tabs = Rect::from_min_size(Pos2::new(inner.min.x, head.max.y + 4.0), Vec2::new(inner.width(), 42.0));
        painter.rect_filled(tabs, CornerRadius::same(14), theme::TRACK);
        let tab_width = (tabs.width() - 8.0) / 4.0;
        let definitions = [
            (InspectorTab::Transform, "Transform"),
            (InspectorTab::Colors, "Colors"),
            (InspectorTab::Effects, "Effects"),
            (InspectorTab::Text, "Text"),
        ];
        for (index, (tab, label)) in definitions.into_iter().enumerate() {
            let tab_rect = Rect::from_min_size(Pos2::new(tabs.min.x + 4.0 + index as f32 * tab_width, tabs.min.y + 4.0), Vec2::new(tab_width, tabs.height() - 8.0));
            if self.inspector_tab == tab { painter.rect_filled(tab_rect, CornerRadius::same(11), theme::TOOL_ACTIVE); }
            painter.text(tab_rect.center(), Align2::CENTER_CENTER, label, theme::font(theme::FONT_TINY), if self.inspector_tab == tab { theme::TEXT } else { theme::MUTED });
            if ui.interact(tab_rect, egui::Id::new(("inspector-tab", index)), Sense::click()).clicked() { self.inspector_tab = tab; }
        }

        let content = Rect::from_min_max(Pos2::new(inner.min.x, tabs.max.y + 10.0), inner.max);
        let tab = self.inspector_tab;
        ui.scope_builder(egui::UiBuilder::new().max_rect(content), |ui| {
            ui.set_clip_rect(content);
            ui.visuals_mut().widgets.inactive.bg_fill = theme::FIELD;
            ui.visuals_mut().widgets.hovered.bg_fill = theme::TOOL_ACTIVE;
            ui.visuals_mut().widgets.active.bg_fill = theme::TOOL_ACTIVE;
            ui.visuals_mut().widgets.inactive.fg_stroke.color = theme::TEXT;
            ui.visuals_mut().widgets.hovered.fg_stroke.color = theme::TEXT;
            ui.visuals_mut().widgets.active.fg_stroke.color = theme::TEXT;
            egui::ScrollArea::vertical().id_salt("inspector-scroll").show(ui, |ui| {
                ui.set_min_width(content.width() - 10.0);
                if self.selected_clip.is_none() {
                    ui.add_space(44.0);
                    ui.vertical_centered(|ui| {
                        icons::draw_at(ui.painter(), ui.max_rect().center_top() + Vec2::new(0.0, 12.0), 28.0, "film", theme::MUTED);
                        ui.add_space(45.0);
                        ui.label(egui::RichText::new("Nothing to adjust").strong().color(theme::TEXT));
                        ui.label(egui::RichText::new("Select a clip on the timeline to edit it.").color(theme::MUTED));
                    });
                    return;
                }
                let selected_kind = self.selected_clip.as_deref()
                    .and_then(|id| self.timeline.clips().iter().find(|clip| clip.id == id));
                let audio_only = selected_kind.is_some_and(|clip| !clip.track.is_video());
                let video_has_audio = selected_kind.is_some_and(|clip| clip.track.is_video() && clip.extra.get("hasAudio").and_then(serde_json::Value::as_bool) == Some(true));
                match tab {
                    InspectorTab::Transform if audio_only => self.draw_audio_controls(ui),
                    InspectorTab::Transform => {
                        self.draw_transform_controls(ui);
                        if video_has_audio {
                            ui.add_space(10.0);
                            self.draw_audio_controls(ui);
                        }
                    }
                    InspectorTab::Colors => self.draw_color_controls(ui),
                    InspectorTab::Effects => self.draw_effect_controls(ui),
                    InspectorTab::Text => self.draw_text_controls(ui),
                }
            });
        });
    }

    fn draw_transform_controls(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Position").strong().color(theme::TEXT));
            if ui.small_button("Reset").clicked() { self.reset_transform_group("position"); }
        });
        self.draw_number_row(ui, "X", &["transform", "position", "x"], 0.0, " px");
        self.draw_number_row(ui, "Y", &["transform", "position", "y"], 0.0, " px");

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Scale").strong().color(theme::TEXT));
            if ui.small_button("Reset").clicked() { self.reset_transform_group("scale"); }
            let mut linked = self.property_bool(&["transform", "scale", "linked"], true);
            if ui.checkbox(&mut linked, "Link").changed() {
                self.set_property_value(&["transform", "scale", "linked"], serde_json::json!(linked));
            }
        });
        self.draw_scale_row(ui, "W", "w");
        self.draw_scale_row(ui, "H", "h");

        ui.add_space(10.0);
        self.draw_slider(ui, "Rotation", &["transform", "rotation"], -180.0, 180.0, 0.0, "°");
        self.draw_slider(ui, "Opacity", &["transform", "opacity"], 0.0, 100.0, 100.0, "%");
        ui.add_space(12.0);
        ui.label(egui::RichText::new("Crop").strong().color(theme::TEXT));
        for axis in ["top", "right", "bottom", "left"] {
            self.draw_slider(ui, axis, &["transform", "crop", axis], 0.0, 100.0, 0.0, " px");
        }
    }

    fn draw_color_controls(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Light").strong().color(theme::TEXT));
        for (label, key, min, max) in [("Exposure", "exposure", -5.0, 5.0), ("Brightness", "brightness", -100.0, 100.0), ("Contrast", "contrast", -100.0, 100.0)] {
            self.draw_slider(ui, label, &["colors", "light", key], min, max, 0.0, "");
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Color").strong().color(theme::TEXT));
        for (label, key, min, max) in [("Saturation", "saturation", -100.0, 100.0), ("Vibrance", "vibrance", -100.0, 100.0), ("Temperature", "temperature", -100.0, 100.0)] {
            self.draw_slider(ui, label, &["colors", "color", key], min, max, 0.0, "");
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Detail").strong().color(theme::TEXT));
        for (label, key, min, max) in [("Clarity", "clarity", -100.0, 100.0), ("Sharpness", "sharpness", 0.0, 100.0), ("Vignette", "vignette", -100.0, 100.0)] {
            self.draw_slider(ui, label, &["colors", "detail", key], min, max, 0.0, "");
        }
    }

    fn draw_audio_controls(&mut self, ui: &mut Ui) {
        ui.label(egui::RichText::new("Audio").strong().color(theme::TEXT));
        self.draw_slider(ui, "Volume", &["audio", "volume"], 0.0, 200.0, 100.0, "%");
        self.draw_slider(ui, "Pan", &["audio", "pan"], -100.0, 100.0, 0.0, "");
        self.draw_slider(ui, "Bass", &["audio", "bass"], -24.0, 24.0, 0.0, " dB");
        self.draw_slider(ui, "Treble", &["audio", "treble"], -24.0, 24.0, 0.0, " dB");
        let mut fade_in = self.property_bool(&["audio", "fadeIn"], false);
        let mut fade_out = self.property_bool(&["audio", "fadeOut"], false);
        ui.horizontal(|ui| {
            if ui.checkbox(&mut fade_in, "Fade in").changed() {
                self.set_property_value(&["audio", "fadeIn"], serde_json::json!(fade_in));
            }
            if ui.checkbox(&mut fade_out, "Fade out").changed() {
                self.set_property_value(&["audio", "fadeOut"], serde_json::json!(fade_out));
            }
        });
        if fade_in { self.draw_slider(ui, "Fade in duration", &["audio", "fadeInDuration"], 0.0, 10.0, 1.0, " s"); }
        if fade_out { self.draw_slider(ui, "Fade out duration", &["audio", "fadeOutDuration"], 0.0, 10.0, 1.0, " s"); }
    }

    fn draw_effect_controls(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        self.draw_transition_controls(ui);
        ui.add_space(12.0);
        ui.label(egui::RichText::new("Filter presets").strong().color(theme::TEXT));
        let selected = self.property_string(&["effects", "preset", "id"], "none");
        let mut preset = selected.clone();
        ui.horizontal_wrapped(|ui| {
            for (id, label) in [("none", "None"), ("vivid", "Vivid"), ("noir", "Noir"), ("fade", "Fade"), ("warm", "Warm"), ("cool", "Cool"), ("cinema", "Cinema"), ("retro", "Retro")] {
                if ui.selectable_value(&mut preset, id.to_owned(), label).clicked() {
                    self.set_property_value(&["effects", "preset", "id"], serde_json::json!(id));
                }
            }
        });
        if selected != "none" { self.draw_slider(ui, "Preset intensity", &["effects", "preset", "intensity"], 0.0, 100.0, 100.0, "%"); }
        ui.add_space(10.0);
        for (group, label, controls) in effect_definitions().iter().copied() {
            let mut enabled = self.property_bool(&["effects", group, "enabled"], false);
            ui.horizontal(|ui| {
                if ui.checkbox(&mut enabled, "").changed() {
                    self.set_property_value(&["effects", group, "enabled"], serde_json::json!(enabled));
                }
                ui.label(egui::RichText::new(label).strong().color(theme::TEXT));
            });
            if enabled {
                for (key, title, min, max, default, unit) in controls {
                    self.draw_slider(ui, title, &["effects", group, key], *min, *max, *default, unit);
                }
                ui.add_space(8.0);
            }
        }
    }

    fn draw_transition_controls(&mut self, ui: &mut Ui) {
        ui.label(egui::RichText::new("Transition to next clip").strong().color(theme::TEXT));
        let Some(id) = self.selected_clip.as_deref() else { return };
        let Some(source) = self.timeline.clips().iter().find(|clip| clip.id == id).cloned() else { return };
        if !source.track.is_video() {
            ui.label(egui::RichText::new("Transitions apply to video clips.").small().color(theme::MUTED));
            return;
        }
        let neighbor = self.timeline.adjacent_clip(id, true).ok().flatten().map(|clip| clip.label.clone());
        let Some(neighbor) = neighbor else {
            ui.label(egui::RichText::new("Place a clip immediately after this one on the same track.").small().color(theme::MUTED));
            return;
        };
        ui.label(egui::RichText::new(format!("{}  →  {}", truncate(&source.label, 18), truncate(&neighbor, 18))).small().color(theme::MUTED));
        let current = source.extra.get("trans");
        let current_type = current.and_then(|value| value.get("type")).and_then(serde_json::Value::as_str).unwrap_or("none");
        let mut kind = current_type.to_owned();
        let mut changed = false;
        ui.horizontal_wrapped(|ui| {
            for (value, label) in [("none", "None"), ("dissolve", "Cross dissolve"), ("dip", "Dip to black"), ("wipe", "Wipe"), ("iris", "Iris")] {
                changed |= ui.selectable_value(&mut kind, value.to_owned(), label).changed();
            }
        });
        let mut duration = current.and_then(|value| value.get("dur")).and_then(serde_json::Value::as_f64).unwrap_or(0.5);
        let mut duration_changed = false;
        if kind != "none" {
            duration_changed = ui.add(egui::Slider::new(&mut duration, 0.05..=3.0).text("Duration")).changed();
        }
        if changed || duration_changed { self.set_transition(&kind, duration); }
    }

    fn draw_text_controls(&mut self, ui: &mut Ui) {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("Title").strong().color(theme::TEXT));
        self.draw_text_field(ui, "Main text", &["text", "title"]);
        self.draw_text_field(ui, "Subtitle", &["text", "sub"]);
        self.draw_text_field(ui, "Caption", &["text", "caption"]);
        self.draw_slider(ui, "Text size", &["text", "size"], 8.0, 400.0, 100.0, "%");
        ui.label(egui::RichText::new("Alignment").strong().color(theme::TEXT));
        let mut alignment = self.property_string(&["text", "align"], "center");
        ui.horizontal(|ui| {
            for (value, label) in [("left", "Left"), ("center", "Center"), ("right", "Right")] {
                if ui.selectable_value(&mut alignment, value.to_owned(), label).changed() {
                    self.set_property_value(&["text", "align"], serde_json::json!(value));
                }
            }
        });
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Choose the Text tool, then drag a box in the preview to place a title.").small().color(theme::MUTED));
    }

    fn draw_number_row(&mut self, ui: &mut Ui, label: &str, path: &[&str], default: f64, unit: &str) {
        let mut value = self.property_number(path, default);
        let response = ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).color(theme::MUTED));
            ui.add(egui::DragValue::new(&mut value).speed(0.5).suffix(unit))
        }).inner;
        if response.changed() { self.set_number(path, value); }
    }

    fn draw_scale_row(&mut self, ui: &mut Ui, label: &str, axis: &str) {
        let mut value = self.property_number(&["transform", "scale", axis], 100.0);
        let response = ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).color(theme::MUTED));
            ui.add(egui::DragValue::new(&mut value).speed(0.5).suffix(" %"))
        }).inner;
        if response.changed() { self.set_scale_axis(axis, value); }
    }

    fn draw_slider(&mut self, ui: &mut Ui, label: &str, path: &[&str], min: f64, max: f64, default: f64, unit: &str) {
        let mut value = self.property_number(path, default) as f32;
        let changed = ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).color(theme::TEXT));
            let slider_width = (ui.available_width() - 70.0).max(48.0);
            let slider = ui.add_sized(
                [slider_width, 18.0],
                egui::Slider::new(&mut value, min as f32..=max as f32).show_value(false),
            );
            let number = ui.add(
                egui::DragValue::new(&mut value)
                    .speed((max - min).abs() as f32 / 500.0)
                    .suffix(unit),
            );
            slider.changed() || number.changed()
        }).inner;
        if changed { self.set_number(path, value as f64); }
    }

    fn draw_text_field(&mut self, ui: &mut Ui, label: &str, path: &[&str]) {
        let mut value = self.property_string(path, "");
        ui.label(egui::RichText::new(label).color(theme::MUTED));
        if ui.text_edit_singleline(&mut value).changed() {
            self.set_property_value(path, serde_json::json!(value));
        }
    }

    // -- timeline -----------------------------------------------------------

    fn timeline(&mut self, ui: &mut Ui, rect: Rect) {
        let painter = ui.painter().clone();
        theme::card(&painter, rect);

        // `.ed-tl-bar`
        let bar_h = theme::TL_BAR_PAD_Y * 2.0 + 24.0;
        let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), bar_h));
        theme::hairline(
            &painter,
            Rect::from_min_size(Pos2::new(bar.min.x, bar.max.y - 1.0), Vec2::new(bar.width(), 1.0)),
            theme::BORDER,
        );

        let tools = [
            (Tool::Cursor, "cursor"),
            (Tool::Hand, "hand"),
            (Tool::Razor, "scissors"),
            (Tool::Text, "type"),
        ];
        let mut x = bar.min.x + theme::TL_BAR_PAD_X;
        for (tool, icon) in tools {
            let b = Rect::from_center_size(
                Pos2::new(x + 19.0, bar.center().y),
                Vec2::splat(38.0),
            );
            let active = self.tool == tool;
            let resp = ui.interact(b, egui::Id::new(("tl-tool", icon)), Sense::click());
            if active {
                painter.rect_filled(b, CornerRadius::same(13), theme::TOOL_ACTIVE);
            } else if resp.hovered() {
                painter.rect_filled(b, CornerRadius::same(13), theme::HOVER);
            }
            icons::draw_at(
                &painter,
                b.center(),
                20.0,
                icon,
                if active { theme::TEXT } else { theme::MUTED },
            );
            if resp.clicked() {
                self.tool = tool;
            }
            x += 38.0 + 6.0;
        }

        // Magnet toggle
        x += 4.0;
        let mag = Rect::from_center_size(
            Pos2::new(x + 19.0, bar.center().y),
            Vec2::splat(38.0),
        );
        let mag_resp = ui.interact(mag, egui::Id::new("tl-magnet"), Sense::click());
        if self.magnet {
            painter.rect_filled(mag, CornerRadius::same(13), theme::BLUE_SOFT);
        } else if mag_resp.hovered() {
            painter.rect_filled(mag, CornerRadius::same(13), theme::HOVER);
        }
        icons::draw_at(
            &painter,
            mag.center(),
            20.0,
            "magnet",
            if self.magnet { theme::BLUE } else { theme::MUTED },
        );
        if mag_resp.clicked() {
            self.magnet = !self.magnet;
        }

        // Timeline zoom controls.
        let zoom_in = Rect::from_center_size(Pos2::new(bar.max.x - 30.0, bar.center().y), Vec2::splat(32.0));
        let zoom_out = Rect::from_center_size(Pos2::new(zoom_in.min.x - 32.0, bar.center().y), Vec2::splat(32.0));
        if self.icon_button(ui, &painter, zoom_out, "tl-zoom-out", "zoomOut", 17.0, theme::MUTED).clicked() {
            self.zoom = (self.zoom / 1.2).clamp(0.25, 12.0);
        }
        if self.icon_button(ui, &painter, zoom_in, "tl-zoom-in", "zoomIn", 17.0, theme::MUTED).clicked() {
            self.zoom = (self.zoom * 1.2).clamp(0.25, 12.0);
        }
        let has_keyframes = self.selected_clip.as_deref().and_then(|id| self.timeline.clips().iter().find(|clip| clip.id == id))
            .and_then(|clip| clip.extra.get("keyframes")).and_then(serde_json::Value::as_array).is_some_and(|keys| !keys.is_empty());
        let key_group = Rect::from_min_size(
            Pos2::new(zoom_out.min.x - 104.0, bar.center().y - 16.0),
            Vec2::new(96.0, 32.0),
        );
        painter.rect_filled(key_group, CornerRadius::same(9), theme::TRACK);
        for (index, (icon, label)) in [("keyPrev", "Previous keyframe"), ("keyAdd", "Add keyframe"), ("keyNext", "Next keyframe")].into_iter().enumerate() {
            let button = Rect::from_min_size(Pos2::new(key_group.min.x + 2.0 + index as f32 * 31.0, key_group.min.y + 2.0), Vec2::splat(28.0));
            let tint = if index == 1 || has_keyframes { theme::MUTED } else { theme::RAIL };
            let response = self.icon_button(ui, &painter, button, ("tl-keyframe", index), icon, 17.0, tint).on_hover_text(label);
            if response.clicked() {
                match index {
                    0 if has_keyframes => self.seek_keyframe(false),
                    1 => self.toggle_keyframe(),
                    2 if has_keyframes => self.seek_keyframe(true),
                    _ => {}
                }
            }
        }

        // `.ed-tl-body { grid-template-columns: 64px 1fr }`
        let body = Rect::from_min_max(
            Pos2::new(rect.min.x, bar.max.y),
            Pos2::new(rect.max.x, rect.max.y),
        );
        let labels = Rect::from_min_size(body.min, Vec2::new(theme::TL_LABEL_COL, body.height()));
        let content = Rect::from_min_max(
            Pos2::new(labels.max.x, body.min.y),
            Pos2::new(body.max.x, body.max.y),
        );
        theme::hairline(
            &painter,
            Rect::from_min_size(Pos2::new(labels.max.x - 1.0, body.min.y), Vec2::new(1.0, body.height())),
            theme::BORDER,
        );

        let used_tracks = self.timeline.visible_tracks();
        let visible: Vec<usize> = self.tracks.iter().enumerate()
            .filter(|(_, track)| track.visible || used_tracks.contains(&track.track_id))
            .map(|(i, _)| i).collect();

        let tracks_h = visible.len() as f32 * theme::TL_TRACK_H;
        let free = body.height() - theme::TL_RULER_H - tracks_h;
        let track_scroll_max = (tracks_h - (body.height() - theme::TL_RULER_H)).max(0.0);
        self.track_scroll = self.track_scroll.clamp(0.0, track_scroll_max);
        let tracks_top = body.min.y + theme::TL_RULER_H + (free * 0.5).max(0.0) - self.track_scroll;

        // Ruler
        let ruler = Rect::from_min_size(
            content.min,
            Vec2::new(content.width(), theme::TL_RULER_H),
        );
        theme::hairline(
            &painter,
            Rect::from_min_size(
                Pos2::new(ruler.min.x, ruler.max.y - 1.0),
                Vec2::new(ruler.width(), 1.0),
            ),
            theme::BORDER,
        );

        let pps = (content.width() / self.duration as f32) * self.zoom;
        let visible_seconds = content.width() / pps.max(0.001);
        let max_timeline_offset = (self.duration as f32 - visible_seconds).max(0.0) as f64;
        self.timeline_offset = self.timeline_offset.min(max_timeline_offset);
        self.timeline_content = content;
        self.track_hit_areas.clear();
        // Pass the horizontal scroll offset explicitly. Capturing the field here
        // would keep an immutable borrow of `self` alive while wheel and middle
        // mouse gestures update the offset below.
        let x_of = |t: f64, offset: f64| content.min.x + (t - offset) as f32 * pps;

        let step = if pps >= 90.0 {
            0.5
        } else if pps >= 45.0 {
            1.0
        } else if pps >= 20.0 {
            2.0
        } else {
            5.0
        };
        let mut t = (self.timeline_offset / step).floor() * step;
        while t <= self.timeline_offset + visible_seconds as f64 + step {
            let px = x_of(t, self.timeline_offset);
            if px > content.max.x {
                break;
            }
            let per_step = if step < 1.0 { 2i64 } else { 1i64 };
            let major = ((t / step).round() as i64) % per_step == 0;
            let h = if major { 12.0 } else { 6.0 };
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(px, ruler.max.y - 6.0 - h), Vec2::new(1.0, h)),
                CornerRadius::ZERO,
                if major { theme::MUTED } else { theme::BORDER },
            );
            if major {
                painter.text(
                    Pos2::new(px + 4.0, ruler.min.y + 3.0),
                    Align2::LEFT_TOP,
                    format_seconds(t),
                    theme::font(11.0),
                    theme::MUTED,
                );
            }
            t += step;
        }

        // Track rows
        for (slot, idx) in visible.iter().enumerate() {
            let top = tracks_top + slot as f32 * theme::TL_TRACK_H;
            let row = Rect::from_min_size(
                Pos2::new(content.min.x, top),
                Vec2::new(content.width(), theme::TL_TRACK_H),
            );
            self.track_hit_areas.push((self.tracks[*idx].track_id, row));
            if slot % 2 == 1 {
                painter.rect_filled(row, CornerRadius::ZERO, theme::ZEBRA);
            }
            theme::hairline(
                &painter,
                Rect::from_min_size(
                    Pos2::new(row.min.x, row.max.y - 1.0),
                    Vec2::new(row.width(), 1.0),
                ),
                theme::TRACK_LINE,
            );

            let label = Rect::from_min_size(
                Pos2::new(labels.min.x, top),
                Vec2::new(labels.width(), theme::TL_TRACK_H),
            );
            let track = &self.tracks[*idx];
            let selected = self.selected_track == Some(*idx);
            if selected {
                painter.rect_filled(label, CornerRadius::ZERO, theme::HOVER);
            }
            theme::hairline(
                &painter,
                Rect::from_min_size(
                    Pos2::new(label.min.x, label.max.y - 1.0),
                    Vec2::new(label.width(), 1.0),
                ),
                theme::TRACK_LINE,
            );
            let glyph = match track.kind {
                Kind::Video => "video",
                Kind::Audio => "music",
            };
            icons::draw_at(
                &painter,
                Pos2::new(label.center().x, label.center().y - 6.0),
                14.0,
                glyph,
                if selected { theme::TEXT } else { theme::MUTED },
            );
            painter.text(
                Pos2::new(label.center().x, label.center().y + 11.0),
                Align2::CENTER_CENTER,
                track.id,
                theme::bold(theme::FONT_TINY),
                if selected { theme::TEXT } else { theme::MUTED },
            );
            let resp = ui.interact(
                label,
                egui::Id::new(("tl-label", *idx)),
                Sense::click(),
            );
            if resp.clicked() {
                self.selected_track = Some(*idx);
            }
        }

        // Shift-wheel scrolls left/right. Regular wheel moves the lane stack.
        if content.contains(ui.ctx().input(|i| i.pointer.hover_pos().unwrap_or(Pos2::new(-1.0, -1.0)))) {
            let (delta, shift) = ui.ctx().input(|i| (i.smooth_scroll_delta, i.modifiers.shift));
            if shift {
                self.timeline_offset = (self.timeline_offset - (if delta.y.abs() > delta.x.abs() { delta.y } else { delta.x }) as f64 / f64::from(pps.max(0.001))).max(0.0);
            } else {
                self.track_scroll = (self.track_scroll - delta.y).clamp(0.0, track_scroll_max);
            }
        }
        self.timeline_offset = self.timeline_offset.min((self.duration as f32 - visible_seconds).max(0.0) as f64);

        let (middle_pressed, middle_down, middle_released, pointer_pos) = ui.ctx().input(|i| (
            i.pointer.button_pressed(egui::PointerButton::Middle),
            i.pointer.button_down(egui::PointerButton::Middle),
            i.pointer.button_released(egui::PointerButton::Middle),
            i.pointer.interact_pos(),
        ));
        if middle_pressed {
            if let Some(pos) = pointer_pos.filter(|pos| content.contains(*pos)) {
                self.middle_pan_origin = Some((pos.x, self.timeline_offset));
            }
        }
        if middle_down {
            if let (Some((origin_x, origin_time)), Some(pos)) = (self.middle_pan_origin, pointer_pos) {
                self.timeline_offset = (origin_time - (pos.x - origin_x) as f64 / pps.max(0.001) as f64).clamp(0.0, (self.duration as f32 - visible_seconds).max(0.0) as f64);
            }
        }
        if middle_released { self.middle_pan_origin = None; }

        let (primary_pressed, primary_down, primary_released, primary_pos) = ui.ctx().input(|i| (
            i.pointer.button_pressed(egui::PointerButton::Primary),
            i.pointer.button_down(egui::PointerButton::Primary),
            i.pointer.button_released(egui::PointerButton::Primary),
            i.pointer.interact_pos(),
        ));
        if self.tool == Tool::Hand && primary_pressed {
            if let Some(pos) = primary_pos.filter(|pos| content.contains(*pos)) {
                self.hand_pan_origin = Some((pos.x, self.timeline_offset));
            }
        }
        if self.tool == Tool::Hand && primary_down {
            if let (Some((origin_x, origin_time)), Some(pos)) = (self.hand_pan_origin, primary_pos) {
                self.timeline_offset = (origin_time - (pos.x - origin_x) as f64 / pps.max(0.001) as f64)
                    .clamp(0.0, (self.duration as f32 - visible_seconds).max(0.0) as f64);
            }
        }
        if primary_released { self.hand_pan_origin = None; }

        // Scrub with the cursor/razor tools; the Hand tool reserves left-drag for panning.
        let scrub = Rect::from_min_max(Pos2::new(content.min.x, ruler.min.y), content.max);
        let scrub_sense = if self.tool == Tool::Hand { Sense::hover() } else { Sense::click_and_drag() };
        let resp = ui.interact(scrub, egui::Id::new("tl-scrub"), scrub_sense);
        if self.tool != Tool::Hand && (resp.clicked() || resp.dragged()) {
            if let Some(pos) = ui.ctx().pointer_interact_pos() {
                let t = (self.timeline_offset as f32 + (pos.x - content.min.x) / pps.max(0.001)).clamp(0.0, self.duration as f32);
                self.playhead = t as f64;
            }
        }

        // Drag media from Library onto a lane and move existing clips directly.
        let clips = self.timeline.clips().to_vec();
        let pointer = ui.ctx().pointer_interact_pos();
        for clip in &clips {
            let Some((_, row)) = self.track_hit_areas.iter().find(|(track_id, _)| *track_id == clip.track) else { continue };
            let clip_rect = Rect::from_min_max(
                Pos2::new(x_of(clip.start, self.timeline_offset), row.min.y + 5.0),
                Pos2::new(x_of(clip.end(), self.timeline_offset).max(x_of(clip.start, self.timeline_offset) + 8.0), row.max.y - 5.0),
            );
            if clip_rect.max.x < content.min.x || clip_rect.min.x > content.max.x { continue; }
            let selected = self.selected_clip.as_deref() == Some(clip.id.as_str()) || self.selected_clips.contains(&clip.id);
            let is_audio = !clip.track.is_video();
            let locked = clip.extra.get("locked").and_then(serde_json::Value::as_bool) == Some(true);
            let base = if is_audio { Color32::from_rgb(32, 76, 64) } else { Color32::from_rgb(49, 123, 161) };
            painter.rect_filled(clip_rect, CornerRadius::same(8), base);
            painter.rect_stroke(clip_rect, CornerRadius::same(8), Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { theme::SELECT_RING } else { theme::BORDER }), StrokeKind::Inside);
            icons::draw_at(&painter, Pos2::new(clip_rect.min.x + 14.0, clip_rect.center().y - 5.0), 13.0, if is_audio { "music" } else { "film" }, theme::TEXT);
            painter.text(Pos2::new(clip_rect.min.x + 25.0, clip_rect.center().y - 5.0), Align2::LEFT_CENTER, truncate(&clip.label, ((clip_rect.width() / 7.0).max(4.0)) as usize), theme::bold(theme::FONT_TINY), theme::TEXT);
            let detached_audio = clip.extra.get("audioDetached").and_then(serde_json::Value::as_bool) == Some(true);
            let show_waveform = is_audio || (clip.linked_clip_id.is_none() && !detached_audio);
            if let Some(waveform) = clip.extra.get("waveform").and_then(serde_json::Value::as_array).filter(|waveform| !waveform.is_empty() && show_waveform) {
                let wave_y = clip_rect.center().y + 9.0;
                let count = ((clip_rect.width() / 3.5).max(0.0) as usize).min(512);
                let source_duration = clip.extra.get("sourceDuration").and_then(serde_json::Value::as_f64).unwrap_or(clip.duration).max(0.001);
                let source_offset = clip.extra.get("sourceOffset").and_then(serde_json::Value::as_f64).unwrap_or(0.0).max(0.0);
                let first = ((source_offset / source_duration) * waveform.len() as f64).floor() as usize;
                let span = ((clip.duration / source_duration) * waveform.len() as f64).ceil() as usize;
                for sample in 0..count {
                    let x = clip_rect.min.x + 7.0 + sample as f32 * 3.5;
                    if x >= clip_rect.max.x - 5.0 { break; }
                    let source_index = first + sample.saturating_mul(span) / count.max(1);
                    let h = waveform.get(source_index.min(waveform.len() - 1)).and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32 * 18.0;
                    if h <= 0.5 { continue; }
                    painter.line_segment([Pos2::new(x, wave_y - h * 0.5), Pos2::new(x, wave_y + h * 0.5)], Stroke::new(1.3, Color32::from_rgb(205, 229, 216)));
                }
            }

            if let Some(keyframes) = clip.extra.get("keyframes").and_then(serde_json::Value::as_array) {
                for (key_index, keyframe) in keyframes.iter().enumerate() {
                    let Some(local_time) = keyframe["time"].as_f64().filter(|time| *time >= 0.0 && *time <= clip.duration) else { continue };
                    let key_x = x_of(clip.start + local_time, self.timeline_offset);
                    if key_x < clip_rect.min.x || key_x > clip_rect.max.x { continue; }
                    let frame_id = (local_time * self.fps.max(1.0)).round() as i64;
                    let selection_id = (clip.id.clone(), frame_id);
                    let is_selected_key = self.selected_keyframes.contains(&selection_id);
                    let center = Pos2::new(key_x, clip_rect.center().y);
                    let diamond = Rect::from_center_size(center, Vec2::splat(14.0));
                    painter.add(egui::Shape::convex_polygon(
                        vec![Pos2::new(center.x, diamond.min.y), Pos2::new(diamond.max.x, center.y), Pos2::new(center.x, diamond.max.y), Pos2::new(diamond.min.x, center.y)],
                        if is_selected_key { Color32::from_rgb(224, 193, 255) } else { Color32::from_rgb(147, 83, 205) },
                        Stroke::new(1.0, Color32::from_rgb(25, 19, 33)),
                    ));
                    let key_sense = if self.tool == Tool::Cursor { Sense::click_and_drag() } else { Sense::hover() };
                    let key_response = ui.interact(diamond.expand(3.0), egui::Id::new(("timeline-keyframe", clip.id.as_str(), key_index)), key_sense);
                    let easing = keyframe["easing"].as_str().unwrap_or("linear");
                    let mut easing_choice = None;
                    key_response.clone().context_menu(|menu| {
                        menu.visuals_mut().widgets.inactive.bg_fill = theme::FIELD;
                        menu.visuals_mut().widgets.hovered.bg_fill = theme::TOOL_ACTIVE;
                        menu.visuals_mut().widgets.active.bg_fill = theme::TOOL_ACTIVE;
                        menu.visuals_mut().widgets.inactive.fg_stroke.color = theme::TEXT;
                        menu.visuals_mut().widgets.hovered.fg_stroke.color = theme::TEXT;
                        menu.visuals_mut().widgets.active.fg_stroke.color = theme::TEXT;
                        menu.label(egui::RichText::new("KEYFRAME EASING").small().color(theme::MUTED));
                        for (value, label) in [("linear", "Linear"), ("ease-in", "Ease in"), ("ease-out", "Ease out"), ("ease-in-out", "Ease in / out"), ("hold", "Hold")]
                        {
                            if menu.selectable_label(easing == value, label).clicked() { easing_choice = Some(value); }
                        }
                    });
                    if let Some(easing) = easing_choice {
                        self.set_keyframe_easing(&clip.id, key_index, easing);
                    }
                    if key_response.clicked() {
                        let shift = ui.ctx().input(|input| input.modifiers.shift);
                        self.selected_clip = Some(clip.id.clone());
                        self.playing = false;
                        self.playhead = clip.start + local_time;
                        if shift {
                            if !self.selected_keyframes.remove(&selection_id) { self.selected_keyframes.insert(selection_id.clone()); }
                        } else {
                            self.selected_keyframes.clear();
                            self.selected_keyframes.insert(selection_id.clone());
                        }
                    }
                    if key_response.drag_started() {
                        let shift = ui.ctx().input(|input| input.modifiers.shift);
                        self.selected_clip = Some(clip.id.clone());
                        if shift {
                            if !self.selected_keyframes.contains(&selection_id) { self.selected_keyframes.insert(selection_id.clone()); }
                        } else if !self.selected_keyframes.contains(&selection_id) {
                            self.selected_keyframes.clear();
                            self.selected_keyframes.insert(selection_id.clone());
                        }
                        let mut indices: HashSet<usize> = keyframes.iter().enumerate().filter_map(|(index, candidate)| {
                            let time = candidate["time"].as_f64()?;
                            let key = (clip.id.clone(), (time * self.fps.max(1.0)).round() as i64);
                            self.selected_keyframes.contains(&key).then_some(index)
                        }).collect();
                        indices.insert(key_index);
                        self.push_undo();
                        self.keyframe_drag = Some(KeyframeDrag {
                            clip_id: clip.id.clone(),
                            origin_x: key_x,
                            pixels_per_second: pps,
                            original: keyframes.clone(),
                            indices,
                        });
                    }
                    if key_response.dragged() {
                        if let Some(pointer) = pointer { self.apply_keyframe_drag(pointer.x); }
                    }
                    if key_response.drag_stopped() { self.keyframe_drag = None; }
                }
            }

            let trim_width = 8.0_f32.min((clip_rect.width() * 0.25).max(3.0));
            let left_trim = Rect::from_min_size(clip_rect.min, Vec2::new(trim_width, clip_rect.height()));
            let right_trim = Rect::from_min_size(Pos2::new(clip_rect.max.x - trim_width, clip_rect.min.y), Vec2::new(trim_width, clip_rect.height()));
            if selected {
                painter.rect_filled(Rect::from_min_size(Pos2::new(clip_rect.min.x + 2.0, clip_rect.center().y - 6.0), Vec2::new(2.0, 12.0)), CornerRadius::same(1), Color32::from_white_alpha(190));
                painter.rect_filled(Rect::from_min_size(Pos2::new(clip_rect.max.x - 4.0, clip_rect.center().y - 6.0), Vec2::new(2.0, 12.0)), CornerRadius::same(1), Color32::from_white_alpha(190));
            }
            let offset = clip.extra.get("sourceOffset").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
            for (left_edge, handle) in [(true, left_trim), (false, right_trim)] {
                let trim_sense = if self.tool == Tool::Cursor { Sense::drag() } else { Sense::hover() };
                let trim_response = ui.interact(handle, egui::Id::new(("timeline-trim", clip.id.as_str(), left_edge)), trim_sense);
                if trim_response.drag_started() && !locked {
                    self.selected_clip = Some(clip.id.clone());
                    self.push_undo();
                    if let Some(pointer) = pointer {
                        self.clip_trim_drag = Some(ClipTrimDrag {
                            clip_id: clip.id.clone(),
                            linked_id: clip.linked_clip_id.clone(),
                            left_edge,
                            origin_x: pointer.x,
                            pixels_per_second: pps,
                            start: clip.start,
                            duration: clip.duration,
                            source_offset: offset,
                        });
                    }
                }
                if trim_response.dragged() && !locked {
                    if let Some(pointer) = pointer { self.apply_clip_trim(pointer.x); }
                }
                if trim_response.drag_stopped() { self.clip_trim_drag = None; }
            }
            let body_rect = clip_rect.shrink2(Vec2::new(trim_width, 0.0));
            let response = ui.interact(body_rect, egui::Id::new(("timeline-clip", clip.id.as_str())), Sense::click_and_drag());
            if response.secondary_clicked() && !self.selected_clips.contains(&clip.id) {
                self.selected_clips.clear();
                self.selected_clips.insert(clip.id.clone());
                self.selected_clip = Some(clip.id.clone());
                self.selected_keyframes.clear();
            }
            let has_audio = clip.extra.get("hasAudio").and_then(serde_json::Value::as_bool) == Some(true);
            let linked = clip.linked_clip_id.is_some();
            let grouped = clip.extra.get("groupId").and_then(serde_json::Value::as_str).is_some();
            let can_group = self.selected_clips.len() > 1 || grouped;
            let group_label = if grouped { "Ungroup clips" } else { "Group selected clips" };
            let mut context_action = None;
            response.clone().context_menu(|menu| {
                menu.visuals_mut().widgets.inactive.bg_fill = theme::FIELD;
                menu.visuals_mut().widgets.hovered.bg_fill = theme::TOOL_ACTIVE;
                menu.visuals_mut().widgets.active.bg_fill = theme::TOOL_ACTIVE;
                menu.visuals_mut().widgets.inactive.fg_stroke.color = theme::TEXT;
                menu.visuals_mut().widgets.hovered.fg_stroke.color = theme::TEXT;
                menu.visuals_mut().widgets.active.fg_stroke.color = theme::TEXT;
                menu.set_min_width(176.0);
                menu.label(egui::RichText::new(&clip.label).small().color(theme::MUTED));
                if has_audio && !linked && !is_audio && menu.button("Separate audio").clicked() { context_action = Some("separate"); }
                if linked && menu.button("Unlink audio and video").clicked() { context_action = Some("unlink"); }
                if menu.button(if locked { "Unlock clip" } else { "Lock clip" }).clicked() { context_action = Some("lock"); }
                if can_group && menu.button(group_label).clicked() { context_action = Some("group"); }
                menu.separator();
                if menu.button(egui::RichText::new("Delete clip").color(theme::RED)).clicked() { context_action = Some("delete"); }
            });
            match context_action {
                Some("separate") => self.separate_audio(&clip.id),
                Some("unlink") => self.unlink_audio(&clip.id),
                Some("lock") => self.set_clip_locked(&clip.id, !locked),
                Some("group") => self.toggle_selected_group(),
                Some("delete") => {
                    self.push_undo();
                    if self.timeline.delete_clip(&clip.id).is_ok() {
                        if self.selected_clip.as_deref() == Some(clip.id.as_str()) { self.selected_clip = None; }
                        self.selected_clips.clear();
                        self.selected_keyframes.clear();
                        self.refresh_timeline_duration();
                    }
                }
                _ => {}
            }
            if response.clicked() {
                if self.tool == Tool::Razor {
                    if let Some(pos) = pointer {
                        let at = self.timeline_offset + ((pos.x - content.min.x) / pps.max(0.001)) as f64;
                        self.split_clip_at(&clip.id, at);
                    }
                } else if self.tool != Tool::Hand && self.tool != Tool::Text {
                    let shift = ui.ctx().input(|input| input.modifiers.shift);
                    if shift {
                        if self.selected_clips.remove(&clip.id) {
                            if self.selected_clip.as_deref() == Some(clip.id.as_str()) {
                                self.selected_clip = self.selected_clips.iter().next().cloned();
                            }
                        } else {
                            self.selected_clips.insert(clip.id.clone());
                            self.selected_clip = Some(clip.id.clone());
                        }
                    } else {
                        self.selected_clips.clear();
                        self.selected_clips.insert(clip.id.clone());
                        self.selected_clip = Some(clip.id.clone());
                    }
                    if self.selected_clip.as_deref() != Some(clip.id.as_str()) { self.selected_keyframes.clear(); }
                }
            }
            if response.drag_started() && self.tool == Tool::Cursor && !locked {
                if !self.selected_clips.contains(&clip.id) {
                    self.selected_clips.clear();
                    self.selected_clips.insert(clip.id.clone());
                }
                self.selected_clip = Some(clip.id.clone());
                let ids = self.selected_clip_ids();
                let drag_clips: Vec<Clip> = self.timeline.clips().iter().filter(|candidate| ids.contains(&candidate.id)).cloned().collect();
                if drag_clips.iter().any(|candidate| candidate.extra.get("locked").and_then(serde_json::Value::as_bool) == Some(true)) {
                    self.clip_drag_origin = None;
                } else if let Some(pointer) = pointer {
                    self.clip_drag_origin = Some(ClipDrag {
                        primary_id: clip.id.clone(),
                        origin_x: pointer.x,
                        pixels_per_second: pps,
                        primary_start: clip.start,
                        primary_track: clip.track,
                        clips: drag_clips,
                    });
                    self.push_undo();
                }
            }
            if response.dragged() && self.tool == Tool::Cursor && !locked {
                let drag = self.clip_drag_origin.clone();
                if let (Some(drag), Some(pointer)) = (drag, pointer) {
                    if drag.primary_id == clip.id {
                        let target = self.track_hit_areas.iter().find(|(_, track_row)| track_row.contains(pointer)).map(|(track_id, _)| *track_id).unwrap_or(clip.track);
                        let group_move = self.selected_clips.len() > 1 || clip.extra.get("groupId").is_some();
                        if group_move {
                            self.apply_clip_group_drag(pointer.x, target);
                        } else {
                            let desired = (drag.primary_start + (pointer.x - drag.origin_x) as f64 / drag.pixels_per_second.max(0.001) as f64).max(0.0);
                            let mut timeline = self.timeline.clone();
                            timeline.snapping = self.magnet;
                            if timeline.move_clip(&clip.id, desired, target, self.playhead, (8.0 / pps.max(0.001)) as f64).is_ok() {
                                self.timeline = timeline;
                                self.refresh_timeline_duration();
                            }
                        }
                    }
                }
            }
            if response.drag_stopped() { self.clip_drag_origin = None; }
        }

        // Playhead
        let px = x_of(self.playhead, self.timeline_offset);
        if px >= content.min.x && px <= content.max.x {
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(px - 0.75, ruler.min.y + 4.0), Vec2::new(1.5, body.height() - 4.0)),
                CornerRadius::ZERO,
                theme::RED,
            );
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(px - 5.0, ruler.min.y),
                    Pos2::new(px + 5.0, ruler.min.y),
                    Pos2::new(px, ruler.min.y + 6.0),
                ],
                theme::RED,
                Stroke::NONE,
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_logo(ctx: &egui::Context) -> Option<egui::TextureHandle> {
    let img = image::load_from_memory_with_format(LOGO_ICO, image::ImageFormat::Ico)
        .ok()?
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture("pluto-logo", color, egui::TextureOptions::LINEAR))
}

fn start_preview_worker(ctx: egui::Context) -> (Sender<PreviewCommand>, Receiver<PreviewFrame>) {
    let (command_tx, command_rx) = mpsc::channel::<PreviewCommand>();
    let (frame_tx, frame_rx) = mpsc::sync_channel::<PreviewFrame>(2);
    thread::spawn(move || {
        let mut pending = command_rx.recv().ok();
        while let Some(command) = pending.take() {
            match command {
                PreviewCommand::Stop => pending = command_rx.recv().ok(),
                PreviewCommand::Decode { request_id, path, source_time, continuous, loop_image, filter_graph } => {
                    let time = format!("{:.5}", source_time.max(0.0));
                    let mut ffmpeg = Command::new(media_tool("ffmpeg"));
                    ffmpeg.args(["-hide_banner", "-loglevel", "error"]);
                    if loop_image { ffmpeg.args(["-loop", "1", "-framerate", "30"]); }
                    if continuous { ffmpeg.arg("-re"); }
                    ffmpeg.args(["-ss", time.as_str(), "-i"]).arg(&path)
                        .args(["-filter_complex"]).arg(filter_graph)
                        .args(["-map", "[out]", "-an", "-f", "rawvideo", "-pix_fmt", "rgba"]);
                    if !continuous { ffmpeg.args(["-frames:v", "1"]); }
                    ffmpeg.args(["pipe:1"]).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());

                    let Ok(mut child) = ffmpeg.spawn() else {
                        pending = command_rx.recv().ok();
                        continue;
                    };
                    let Some(mut stdout) = child.stdout.take() else {
                        let _ = child.kill();
                        let _ = child.wait();
                        pending = command_rx.recv().ok();
                        continue;
                    };
                    let mut raw = vec![0_u8; PREVIEW_WIDTH * PREVIEW_HEIGHT * 4];
                    let mut superseding = None;
                    loop {
                        match command_rx.try_recv() {
                            Ok(next) => { superseding = Some(next); break; }
                            Err(TryRecvError::Disconnected) => {
                                let _ = child.kill();
                                let _ = child.wait();
                                return;
                            }
                            Err(TryRecvError::Empty) => {}
                        }
                        if stdout.read_exact(&mut raw).is_err() { break; }
                        if frame_tx.try_send(PreviewFrame { request_id, rgba: raw.clone() }).is_ok() { ctx.request_repaint(); }
                        if !continuous { break; }
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    pending = superseding.or_else(|| command_rx.recv().ok());
                }
            }
        }
    });
    (command_tx, frame_rx)
}

fn set_json_path(value: &mut serde_json::Value, path: &[&str], replacement: serde_json::Value) {
    let Some((head, tail)) = path.split_first() else { return };
    if !value.is_object() { *value = serde_json::json!({}); }
    let object = value.as_object_mut().expect("the JSON value was made an object above");
    if tail.is_empty() {
        object.insert((*head).to_owned(), replacement);
    } else {
        let next = object.entry((*head).to_owned()).or_insert_with(|| serde_json::json!({}));
        set_json_path(next, tail, replacement);
    }
}

fn json_at_path<'a>(mut value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    for part in path { value = value.get(*part)?; }
    Some(value)
}

fn active_keyframe_index(clip: &Clip, local_time: f64, fps: f64) -> Option<usize> {
    let tolerance = 0.5 / fps.max(1.0);
    clip.extra.get("keyframes")?.as_array()?.iter().position(|keyframe| {
        keyframe["time"].as_f64().is_some_and(|time| (time - local_time).abs() <= tolerance)
    })
}

fn clip_property_store_mut(clip: &mut Clip, local_time: f64, fps: f64) -> &mut serde_json::Value {
    // Resolve which store to edit before taking any mutable borrow, so the
    // keyframe borrow and the clip-level `props` borrow never overlap.
    let keyframe_index = active_keyframe_index(clip, local_time, fps).filter(|&index| {
        clip.extra
            .get("keyframes")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|keyframes| index < keyframes.len())
    });
    match keyframe_index {
        Some(index) => {
            let keyframe = &mut clip.extra
                .entry("keyframes".to_owned())
                .or_insert_with(|| serde_json::json!([]))
                [index];
            if !keyframe.get("props").is_some_and(serde_json::Value::is_object) {
                keyframe["props"] = serde_json::json!({});
            }
            &mut keyframe["props"]
        }
        None => clip.extra.entry("props".to_owned()).or_insert_with(|| serde_json::json!({})),
    }
}

type EffectControl = (&'static str, &'static str, f64, f64, f64, &'static str);
type EffectGroup = (&'static str, &'static str, &'static [EffectControl]);

fn effect_definitions() -> &'static [EffectGroup] {
    static BLUR: &[EffectControl] = &[("amount", "Amount", 0.0, 100.0, 25.0, ""), ("angle", "Angle", -180.0, 180.0, 0.0, "°")];
    static GRAIN: &[EffectControl] = &[("amount", "Amount", 0.0, 100.0, 35.0, ""), ("size", "Size", 0.0, 100.0, 30.0, ""), ("roughness", "Roughness", 0.0, 100.0, 50.0, "")];
    static CHROMATIC: &[EffectControl] = &[("amount", "Amount", 0.0, 100.0, 30.0, ""), ("angle", "Angle", -180.0, 180.0, 0.0, "°")];
    static GLOW: &[EffectControl] = &[("amount", "Amount", 0.0, 100.0, 30.0, ""), ("threshold", "Threshold", 0.0, 100.0, 70.0, ""), ("radius", "Radius", 0.0, 100.0, 40.0, "")];
    static GLITCH: &[EffectControl] = &[("amount", "Amount", 0.0, 100.0, 30.0, ""), ("blocks", "Block size", 0.0, 100.0, 25.0, ""), ("speed", "Speed", 0.0, 100.0, 50.0, "")];
    static PIXELATE: &[EffectControl] = &[("size", "Pixel size", 1.0, 80.0, 12.0, " px"), ("blend", "Blend", 0.0, 100.0, 100.0, "%")];
    static SHARPEN: &[EffectControl] = &[("amount", "Amount", 0.0, 100.0, 35.0, ""), ("radius", "Radius", 0.0, 20.0, 5.0, " px"), ("threshold", "Threshold", 0.0, 100.0, 10.0, "")];
    static LENS: &[EffectControl] = &[("amount", "Distortion", -100.0, 100.0, 0.0, ""), ("scale", "Scale", 50.0, 150.0, 100.0, "%"), ("blend", "Blend", 0.0, 100.0, 100.0, "%")];
    static SCANLINES: &[EffectControl] = &[("amount", "Opacity", 0.0, 100.0, 25.0, "%"), ("spacing", "Spacing", 1.0, 12.0, 3.0, " px")];
    static SHAKE: &[EffectControl] = &[("amount", "Intensity", 0.0, 100.0, 20.0, ""), ("frequency", "Frequency", 1.0, 30.0, 8.0, " Hz"), ("smoothness", "Smoothness", 0.0, 100.0, 35.0, "")];
    static FILM_BURN: &[EffectControl] = &[("amount", "Amount", 0.0, 100.0, 22.0, ""), ("warmth", "Warmth", 0.0, 100.0, 65.0, ""), ("edge", "Edge spread", 0.0, 100.0, 38.0, ""), ("flicker", "Flicker", 0.0, 100.0, 12.0, "")];
    static HALFTONE: &[EffectControl] = &[("size", "Dot size", 1.0, 24.0, 5.0, " px"), ("contrast", "Contrast", 0.0, 100.0, 48.0, ""), ("angle", "Angle", 0.0, 180.0, 45.0, "°")];
    static POSTERIZE: &[EffectControl] = &[("levels", "Color levels", 2.0, 16.0, 6.0, ""), ("blend", "Blend", 0.0, 100.0, 100.0, "%")];
    static LIGHT_LEAK: &[EffectControl] = &[("amount", "Intensity", 0.0, 100.0, 28.0, ""), ("position", "Position", 0.0, 100.0, 35.0, ""), ("warmth", "Warmth", 0.0, 100.0, 70.0, "")];
    static MIRROR: &[EffectControl] = &[("blend", "Blend", 0.0, 100.0, 100.0, "%")];
    static EQ: &[EffectControl] = &[("low", "Low", -24.0, 24.0, 0.0, " dB"), ("mid", "Mid", -24.0, 24.0, 0.0, " dB"), ("high", "High", -24.0, 24.0, 0.0, " dB")];
    static REVERB: &[EffectControl] = &[("wet", "Wet", 0.0, 100.0, 25.0, "%"), ("room", "Room", 0.0, 100.0, 50.0, "%"), ("damp", "Damping", 0.0, 100.0, 50.0, "%")];
    static DELAY: &[EffectControl] = &[("time", "Time", 1.0, 2000.0, 320.0, " ms"), ("feedback", "Feedback", 0.0, 95.0, 30.0, "%"), ("mix", "Mix", 0.0, 100.0, 30.0, "%")];
    static COMPRESSOR: &[EffectControl] = &[("threshold", "Threshold", -60.0, 0.0, -18.0, " dB"), ("ratio", "Ratio", 1.0, 20.0, 4.0, ":1"), ("attack", "Attack", 0.0, 200.0, 12.0, " ms"), ("release", "Release", 5.0, 1000.0, 140.0, " ms")];
    static LIMITER: &[EffectControl] = &[("ceiling", "Ceiling", -12.0, 0.0, -1.0, " dB"), ("release", "Release", 1.0, 500.0, 80.0, " ms")];
    static DENOISE: &[EffectControl] = &[("reduction", "Reduction", 0.0, 100.0, 35.0, "%"), ("sensitivity", "Sensitivity", 0.0, 100.0, 50.0, "%")];
    static GROUPS: &[EffectGroup] = &[
        ("blur", "Blur", BLUR), ("grain", "Grain", GRAIN), ("chromatic", "Chromatic aberration", CHROMATIC),
        ("glow", "Glow", GLOW), ("glitch", "Glitch", GLITCH), ("pixelate", "Pixelate", PIXELATE),
        ("sharpen", "Sharpen", SHARPEN), ("lens", "Lens distortion", LENS), ("fisheye", "Fisheye", LENS),
        ("scanlines", "Scanlines", SCANLINES), ("shake", "Camera shake", SHAKE), ("filmBurn", "Film burn", FILM_BURN),
        ("halftone", "Halftone", HALFTONE), ("posterize", "Posterize", POSTERIZE), ("lightLeak", "Light leak", LIGHT_LEAK),
        ("mirror", "Mirror", MIRROR), ("eq", "Equalizer", EQ), ("reverb", "Reverb", REVERB),
        ("delay", "Delay", DELAY), ("compressor", "Compressor", COMPRESSOR), ("limiter", "Limiter", LIMITER),
        ("denoise", "Noise reduction", DENOISE),
    ];
    GROUPS
}

fn load_media_item(id: u64, path: PathBuf) -> Result<RawMediaItem, String> {
    if !path.is_file() {
        return Err(format!("Media file was not found: {}", path.display()));
    }
    let extension = path.extension().and_then(|value| value.to_str()).unwrap_or("").to_ascii_lowercase();
    let is_still = ["png", "jpg", "jpeg", "webp"].contains(&extension.as_str());
    let kind = if is_still {
        Kind::Video
    } else if ["mp3", "wav", "m4a", "aac", "flac", "ogg"].contains(&extension.as_str()) {
        Kind::Audio
    } else if ["mp4", "mov", "mkv", "avi", "webm", "m4v", "gif"].contains(&extension.as_str()) {
        Kind::Video
    } else {
        return Err(format!("Unsupported media type: {}", path.display()));
    };

    let (duration, has_audio) = if is_still {
        (5.0, false)
    } else {
        let probe = Command::new(media_tool("ffprobe"))
            .args(["-v", "error", "-show_entries", "format=duration:stream=codec_type,duration", "-of", "json"])
            .arg(&path)
            .output()
            .map_err(|error| format!("Could not start FFprobe: {error}"))?;
        if !probe.status.success() {
            return Err(String::from_utf8_lossy(&probe.stderr).trim().to_owned());
        }
        let metadata: serde_json::Value = serde_json::from_slice(&probe.stdout)
            .map_err(|error| format!("FFprobe returned invalid media information: {error}"))?;
        let duration = metadata["format"]["duration"].as_str().and_then(|v| v.parse::<f64>().ok())
            .or_else(|| metadata["format"]["duration"].as_f64())
            .filter(|value| value.is_finite() && *value > 0.0)
            .or_else(|| metadata["streams"].as_array().and_then(|streams| streams.iter().filter_map(|stream| {
                stream["duration"].as_str().and_then(|v| v.parse::<f64>().ok()).or_else(|| stream["duration"].as_f64())
            }).reduce(f64::max)))
            .ok_or_else(|| "FFprobe could not determine the media duration".to_owned())?;
        let has_audio = metadata["streams"].as_array().is_some_and(|streams| streams.iter().any(|stream| stream["codec_type"].as_str() == Some("audio")));
        (duration, has_audio)
    };

    let thumbnail_png = if kind == Kind::Audio {
        None
    } else if is_still {
        let output = Command::new(media_tool("ffmpeg"))
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-frames:v", "1", "-vf", "scale=480:270:force_original_aspect_ratio=decrease", "-f", "image2pipe", "-vcodec", "png", "pipe:1"])
            .output()
            .map_err(|error| format!("Could not start FFmpeg to create an image thumbnail: {error}"))?;
        output.status.success().then_some(output.stdout)
    } else {
        let output = Command::new(media_tool("ffmpeg"))
            .args(["-v", "error", "-ss", "0.25", "-i"])
            .arg(&path)
            .args(["-frames:v", "1", "-vf", "scale=480:-2", "-f", "image2pipe", "-vcodec", "png", "pipe:1"])
            .output()
            .map_err(|error| format!("Could not start FFmpeg to create a thumbnail: {error}"))?;
        output.status.success().then_some(output.stdout)
    };
    let thumbnail = thumbnail_png.and_then(|bytes| image::load_from_memory(&bytes).ok())
        .map(|image| image.thumbnail(480, 270).to_rgba8())
        .map(|image| (image.width() as usize, image.height() as usize, image.into_raw()));

    // showwavespic decodes the full audio stream into a small fixed-size image,
    // so long files never create an unbounded PCM buffer in the UI process.
    let waveform = if has_audio {
        let output = Command::new(media_tool("ffmpeg"))
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args([
                "-filter_complex",
                "[0:a:0]showwavespic=s=1024x48:colors=white:scale=sqrt[w]",
                "-map", "[w]", "-frames:v", "1", "-f", "image2pipe", "-vcodec", "png", "pipe:1",
            ])
            .output();
        output.ok().filter(|output| output.status.success())
            .and_then(|output| image::load_from_memory(&output.stdout).ok())
            .map(|image| {
                let image = image.to_luma8();
                (0..image.width()).map(|x| {
                    (0..image.height()).map(|y| image.get_pixel(x, y)[0]).max().unwrap_or(0) as f32 / 255.0
                }).collect()
            }).unwrap_or_default()
    } else {
        Vec::new()
    };

    let name = path.file_name().and_then(|value| value.to_str()).unwrap_or("Media").to_owned();
    Ok(RawMediaItem { id, path, name, duration, kind, is_still, has_audio, thumbnail, waveform })
}

fn start_import_worker(ctx: egui::Context) -> (Sender<(u64, PathBuf)>, Receiver<(PathBuf, Result<RawMediaItem, String>)>) {
    let (command_tx, command_rx) = mpsc::channel::<(u64, PathBuf)>();
    let (event_tx, event_rx) = mpsc::channel::<(PathBuf, Result<RawMediaItem, String>)>();
    thread::spawn(move || {
        while let Ok((id, path)) = command_rx.recv() {
            let result = load_media_item(id, path.clone());
            if event_tx.send((path, result)).is_err() { break; }
            ctx.request_repaint();
        }
    });
    (command_tx, event_rx)
}

fn media_tool(name: &str) -> PathBuf {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    if let Ok(directory) = std::env::var("PLUTO_FFMPEG_DIR") {
        let candidate = PathBuf::from(directory).join(&exe);
        if candidate.is_file() { return candidate; }
    }
    if let Ok(current) = std::env::current_exe() {
        if let Some(dir) = current.parent() {
            for candidate in [dir.join(&exe), dir.join("ffmpeg").join(&exe), dir.join("resources").join("ffmpeg").join(&exe)] {
                if candidate.is_file() { return candidate; }
            }
        }
    }
    PathBuf::from(exe)
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars { return value.to_owned(); }
    let mut result: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    result.push('…');
    result
}

fn safe_filename(value: &str) -> String {
    let name: String = value.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') { '_' } else { c }).collect();
    let trimmed = name.trim_matches(|c: char| c.is_whitespace() || c == '.');
    if trimmed.is_empty() { "Untitled Project".to_owned() } else { trimmed.to_owned() }
}

fn track_lane_index(track: TrackId) -> i32 {
    match track {
        TrackId::V1 | TrackId::A1 => 0,
        TrackId::V2 | TrackId::A2 => 1,
        TrackId::V3 => 2,
        TrackId::V4 => 3,
    }
}

fn track_for_lane(video: bool, lane: i32) -> TrackId {
    match (video, lane.clamp(0, if video { 3 } else { 1 })) {
        (true, 0) => TrackId::V1,
        (true, 1) => TrackId::V2,
        (true, 2) => TrackId::V3,
        (true, _) => TrackId::V4,
        (false, 0) => TrackId::A1,
        (false, _) => TrackId::A2,
    }
}

/// `HH:MM:SS:FF` — the transport uses tabular timecode in the web build.
fn timecode(t: f64, fps: f64) -> String {
    let fps = fps.max(1.0);
    let frames_total = (t.max(0.0) * fps).round() as u64;
    let per_sec = fps.round() as u64;
    let f = frames_total % per_sec;
    let secs = frames_total / per_sec;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60,
        f
    )
}

fn format_seconds(t: f64) -> String {
    if (t.fract()).abs() < 1e-6 {
        format!("{}s", t as i64)
    } else {
        format!("{:.1}s", t)
    }
}
