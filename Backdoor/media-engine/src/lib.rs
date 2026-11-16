use serde_json::Value;
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub type SharedExport = Arc<Mutex<Option<Child>>>;

struct ExportTempFiles(PathBuf);
impl Drop for ExportTempFiles {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn ffmpeg_filter_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").replace(':', "\\:").replace('\'', "\\'")
}
fn multimedia_binary(tool: &str) -> PathBuf {
    let filename = if cfg!(windows) { format!("{tool}.exe") } else { tool.to_owned() };
    if let Ok(directory) = std::env::var("PLUTO_FFMPEG_DIR") {
        let candidate = PathBuf::from(directory).join(&filename);
        if candidate.is_file() { return candidate; }
    }
    if let Ok(executable) = std::env::current_exe() {
        if let Some(directory) = executable.parent() {
            for candidate in [directory.join(&filename), directory.join("ffmpeg").join(&filename), directory.join("resources").join("ffmpeg").join(&filename)] {
                if candidate.is_file() { return candidate; }
            }
        }
    }
    PathBuf::from(filename)
}

fn multimedia_command(tool: &str) -> Command {
    let mut command = Command::new(multimedia_binary(tool));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

pub fn cancel_export(process: &SharedExport) -> Result<(), String> {
    let mut current = process.lock().map_err(|_| "Export process lock failed".to_owned())?;
    if let Some(child) = current.as_mut() { child.kill().map_err(|error| format!("Could not cancel export: {error}"))?; }
    Ok(())
}
fn number(value: &Value, key: &str, fallback: f64) -> f64 { value[key].as_f64().filter(|n| n.is_finite()).unwrap_or(fallback) }
fn enabled(value: &Value, key: &str) -> bool { value[key]["enabled"].as_bool().unwrap_or(false) }
fn n(value: &Value, key: &str, fallback: f64) -> f64 { number(value, key, fallback) }

fn value_at_path<'a>(mut value: &'a Value, path: &[&str]) -> &'a Value {
    for key in path { value = &value[*key]; }
    value
}

fn keyframe_number_expr(clip: &Value, path: &[&str], fallback: f64, start: f64, time_var: &str) -> Option<String> {
    let keyframes = clip["keyframes"].as_array()?;
    if keyframes.is_empty() { return None; }
    let base = value_at_path(&clip["props"], path).as_f64().filter(|n| n.is_finite()).unwrap_or(fallback);
    let mut points = vec![(0.0_f64, base, "linear".to_owned())];
    for keyframe in keyframes {
        let Some(time) = keyframe["time"].as_f64().filter(|n| n.is_finite()) else { continue; };
        if time < 0.0 || time > n(clip, "dur", f64::MAX) { continue; }
        let Some(value) = value_at_path(&keyframe["props"], path).as_f64().filter(|n| n.is_finite()) else { continue; };
        let easing = keyframe["easing"].as_str().unwrap_or("linear").to_owned();
        if let Some(existing) = points.iter_mut().find(|point| (point.0 - time).abs() < 1e-6) {
            *existing = (time, value, easing);
        } else { points.push((time, value, easing)); }
    }
    points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if points.is_empty() { return None; }
    if points.len() == 1 {
        return ((points[0].1 - base).abs() >= 1e-9).then(|| format!("{:.8}", points[0].1));
    }
    if points.windows(2).all(|pair| (pair[0].1 - pair[1].1).abs() < 1e-9) {
        return ((points[0].1 - base).abs() >= 1e-9).then(|| format!("{:.8}", points[0].1));
    }

    let last = points.last()?.1;
    let mut expression = format!("{last:.8}");
    for index in (0..points.len() - 1).rev() {
        let (from_time, from_value, easing) = &points[index];
        let (to_time, to_value, _) = &points[index + 1];
        let from_at = start + *from_time;
        let to_at = start + *to_time;
        let progress = format!("min(max(({time_var}-{from_at:.8})/{:.8},0),1)", (*to_time - *from_time).max(1e-6));
        let eased = match easing.as_str() {
            "ease-in" => format!("({progress})*({progress})"),
            "ease-out" => format!("1-(1-({progress}))*(1-({progress}))"),
            "ease-in-out" => format!("if(lt(({progress}),0.5),2*({progress})*({progress}),1-2*(1-({progress}))*(1-({progress})))"),
            "hold" => "0".to_owned(),
            _ => progress,
        };
        let interpolate = format!("({from_value:.8}+({to_value:.8}-{from_value:.8})*({eased}))");
        expression = format!("if(lt({time_var},{from_at:.8}),{from_value:.8},if(lt({time_var},{to_at:.8}),{interpolate},{expression}))");
    }
    Some(expression)
}

fn video_filters_at(clip: &Value, width: u32, height: u32, start: f64, length: f64, keyframe_start: f64, transition_in: Option<(&str, f64)>, transition_out: Option<(&str, f64)>) -> (String, String, String) {
    let props = &clip["props"];
    let transform = &props["transform"];
    let effects = &props["effects"];
    let colors = &props["colors"];
    let source_start = n(clip, "sourceOffset", 0.0).max(0.0);
    let mut filters = vec![format!("trim=start={source_start:.5}:duration={length:.5}"), format!("setpts=PTS-STARTPTS+{start:.5}/TB"),
        format!("scale={width}:{height}:force_original_aspect_ratio=decrease"),
        format!("pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black@0"), "setsar=1".into(), "format=rgba".into()];

    let crop = &transform["crop"];
    let left = (n(crop, "left", 0.0) / 1920.0).clamp(0.0, 0.45);
    let right = (n(crop, "right", 0.0) / 1920.0).clamp(0.0, 0.45);
    let top = (n(crop, "top", 0.0) / 1080.0).clamp(0.0, 0.45);
    let bottom = (n(crop, "bottom", 0.0) / 1080.0).clamp(0.0, 0.45);
    if left + right > 0.0 || top + bottom > 0.0 {
        filters.push(format!("crop=iw*{:.6}:ih*{:.6}:iw*{left:.6}:ih*{top:.6}", (1.0 - left - right).max(0.1), (1.0 - top - bottom).max(0.1)));
        filters.push(format!("scale={width}:{height}:force_original_aspect_ratio=decrease"));
        filters.push(format!("pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black@0"));
    }

    let light = &colors["light"];
    let color = &colors["color"];
    let detail = &colors["detail"];
    let brightness = (n(light, "exposure", 0.0) * 0.5 + n(light, "brightness", 0.0) / 100.0 * 0.25).clamp(-1.0, 1.0);
    let contrast = (1.0 + n(light, "contrast", 0.0) / 100.0).clamp(0.0, 4.0);
    let saturation = (1.0 + n(color, "saturation", 0.0) / 100.0 + n(color, "vibrance", 0.0) / 150.0).clamp(0.0, 3.0);
    let animated_color = keyframe_number_expr(clip, &["colors", "light", "exposure"], 0.0, keyframe_start, "t").is_some()
        || keyframe_number_expr(clip, &["colors", "light", "brightness"], 0.0, keyframe_start, "t").is_some()
        || keyframe_number_expr(clip, &["colors", "light", "contrast"], 0.0, keyframe_start, "t").is_some()
        || keyframe_number_expr(clip, &["colors", "color", "saturation"], 0.0, keyframe_start, "t").is_some()
        || keyframe_number_expr(clip, &["colors", "color", "vibrance"], 0.0, keyframe_start, "t").is_some();
    if animated_color {
        let exposure = keyframe_number_expr(clip, &["colors", "light", "exposure"], 0.0, keyframe_start, "t").unwrap_or_else(|| format!("{:.8}", n(light, "exposure", 0.0)));
        let light_brightness = keyframe_number_expr(clip, &["colors", "light", "brightness"], 0.0, keyframe_start, "t").unwrap_or_else(|| format!("{:.8}", n(light, "brightness", 0.0)));
        let light_contrast = keyframe_number_expr(clip, &["colors", "light", "contrast"], 0.0, keyframe_start, "t").unwrap_or_else(|| format!("{:.8}", n(light, "contrast", 0.0)));
        let color_saturation = keyframe_number_expr(clip, &["colors", "color", "saturation"], 0.0, keyframe_start, "t").unwrap_or_else(|| format!("{:.8}", n(color, "saturation", 0.0)));
        let vibrance = keyframe_number_expr(clip, &["colors", "color", "vibrance"], 0.0, keyframe_start, "t").unwrap_or_else(|| format!("{:.8}", n(color, "vibrance", 0.0)));
        let brightness_expr = format!("min(max(({exposure})*0.5+({light_brightness})/100*0.25,-1),1)");
        let contrast_expr = format!("min(max(1+({light_contrast})/100,0),4)");
        let saturation_expr = format!("min(max(1+({color_saturation})/100+({vibrance})/150,0),3)");
        filters.push(format!("eq=brightness='{brightness_expr}':contrast='{contrast_expr}':saturation='{saturation_expr}':eval=frame"));
    } else if brightness.abs() > 0.0001 || (contrast - 1.0).abs() > 0.0001 || (saturation - 1.0).abs() > 0.0001 {
        filters.push(format!("eq=brightness={brightness:.5}:contrast={contrast:.5}:saturation={saturation:.5}"));
    }
    let temperature = (n(color, "temperature", 0.0) / 100.0).clamp(-1.0, 1.0);
    if temperature.abs() > 0.01 {
        filters.push(format!("colorbalance=rs={temperature:.4}:bs={:.4}", -temperature));
    }
    for (key, wheel_channel) in [("shadows", "s"), ("tint", "m"), ("highlights", "h")] {
        if let Some(wheel_data) = colors["grading"][key].as_object() {
            let amount = wheel_data.get("amount").and_then(Value::as_f64).unwrap_or(0.0) / 100.0 * 0.3;
            let angle = wheel_data.get("hue").and_then(Value::as_f64).unwrap_or(0.0).to_radians();
            if amount.abs() > 0.001 {
                let r = amount * angle.cos(); let b = amount * angle.sin();
                filters.push(format!("colorbalance=r{wheel_channel}={r:.4}:b{wheel_channel}={b:.4}"));
            }
        }
    }
    let clarity = (n(detail, "clarity", 0.0) / 100.0 * 0.9).clamp(-0.9, 0.9);
    let sharpness = (n(detail, "sharpness", 0.0) / 100.0 * 0.9).clamp(0.0, 0.9);
    let color_detail = (clarity + sharpness).clamp(-0.9, 1.5);
    if color_detail.abs() > 0.001 { filters.push(format!("unsharp=5:5:{color_detail:.4}:5:5:0")); }
    let color_vignette = (n(detail, "vignette", 0.0) / 100.0).clamp(-1.0, 1.0);
    if color_vignette.abs() > 0.01 {
        let edge = "max(0,min(1,(hypot(X-W/2,Y-H/2)/(min(W,H)*0.7)-0.68)/0.32))";
        if color_vignette > 0.0 {
            filters.push(format!("geq=r='min(255,r(X,Y)+255*{:.4}*{edge})':g='min(255,g(X,Y)+255*{:.4}*{edge})':b='min(255,b(X,Y)+255*{:.4}*{edge})':a='alpha(X,Y)'", color_vignette, color_vignette, color_vignette));
        } else {
            filters.push(format!("geq=r='r(X,Y)*(1-{:.4}*{edge})':g='g(X,Y)*(1-{:.4}*{edge})':b='b(X,Y)*(1-{:.4}*{edge})':a='alpha(X,Y)'", color_vignette.abs(), color_vignette.abs(), color_vignette.abs()));
        }
    }

    let preset = effects["preset"]["id"].as_str().unwrap_or("none");
    let intensity = (n(&effects["preset"], "intensity", 100.0) / 100.0).clamp(0.0, 1.0);
    match preset {
        "vivid" => filters.push(format!("eq=saturation={:.3}:contrast=1.08", 1.0 + 0.7 * intensity)),
        "noir" => { filters.push(format!("hue=s={:.3}", 1.0 - intensity)); filters.push(format!("eq=contrast={:.3}:brightness={:.3}", 1.0 + 0.2 * intensity, -0.02 * intensity)); },
        "fade" => filters.push(format!("eq=contrast={:.3}:brightness={:.3}:saturation={:.3}", 1.0 - 0.16 * intensity, 0.08 * intensity, 1.0 - 0.2 * intensity)),
        "warm" => filters.push(format!("colorbalance=rs={:.3}:bs={:.3}", 0.12 * intensity, -0.1 * intensity)),
        "cool" => filters.push(format!("colorbalance=rs={:.3}:bs={:.3}", -0.08 * intensity, 0.12 * intensity)),
        "cinema" => filters.push(format!("eq=contrast={:.3}:saturation={:.3}", 1.0 + 0.15 * intensity, 1.0 - 0.15 * intensity)),
        "retro" => { filters.push(format!("colorbalance=rs={:.3}:bs={:.3}", 0.15 * intensity, -0.12 * intensity)); filters.push(format!("noise=alls={:.2}:allf=t+u", 12.0 * intensity)); },
        _ => {}
    }
    if enabled(effects, "blur") {
        let blur = &effects["blur"];
        let amount = n(blur, "amount", 0.0) * intensity.max(0.01);
        let sigma = (amount * 0.08).max(0.1);
        match blur["mode"].as_str().unwrap_or("gaussian") {
            "motion" => {
                let frames = (2.0 + amount / 14.0).clamp(2.0, 9.0) as u32;
                filters.push(format!("tmix=frames={frames}"));
                let distance = amount * 0.015;
                let angle = n(blur, "angle", 0.0).to_radians();
                filters.push(format!("rgbashift=rh={:.3}:rv={:.3}:bh={:.3}:bv={:.3}", distance * angle.cos(), distance * angle.sin(), -distance * angle.cos(), -distance * angle.sin()));
            }
            "zoom" => {
                filters.push(format!("gblur=sigma={:.3}:steps=2", sigma * 0.7));
                let zoom = (1.0 + amount / 1600.0).clamp(1.0, 1.08);
                filters.push(format!("scale=iw*{zoom:.4}:ih*{zoom:.4}"));
            }
            _ => filters.push(format!("gblur=sigma={sigma:.3}:steps=2")),
        }
    }
    if enabled(effects, "grain") {
        let grain = &effects["grain"];
        let amount = n(grain, "amount", 0.0) * n(grain, "roughness", 50.0) / 50.0 * (0.6 + n(grain, "size", 30.0) / 250.0);
        filters.push(format!("noise=alls={:.2}:allf=t+u", amount.clamp(0.0, 80.0)));
    }
    if enabled(effects, "vignette") {
        let vignette = &effects["vignette"];
        let amount = (n(vignette, "amount", 0.0) / 100.0).clamp(-1.0, 1.0);
        if amount.abs() > 0.01 {
            let midpoint = (n(vignette, "midpoint", 50.0) / 100.0 * 0.8).clamp(0.0, 0.8);
            let feather = (0.08 + n(vignette, "feather", 50.0) / 100.0 * 0.42).clamp(0.04, 0.5);
            let roundness = (1.0 + n(vignette, "roundness", 0.0) / 100.0 * 0.8).clamp(0.2, 2.0);
            let edge = format!("max(0,min(1,(hypot((X-W/2)*{roundness:.4},Y-H/2)/(min(W,H)*0.7)-{midpoint:.4})/{feather:.4}))");
            if amount > 0.0 {
                filters.push(format!("geq=r='min(255,r(X,Y)+255*{:.4}*{edge})':g='min(255,g(X,Y)+255*{:.4}*{edge})':b='min(255,b(X,Y)+255*{:.4}*{edge})':a='alpha(X,Y)'", amount, amount, amount));
            } else {
                filters.push(format!("geq=r='r(X,Y)*(1-{:.4}*{edge})':g='g(X,Y)*(1-{:.4}*{edge})':b='b(X,Y)*(1-{:.4}*{edge})':a='alpha(X,Y)'", amount.abs(), amount.abs(), amount.abs()));
            }
        }
    }
    if enabled(effects, "chromatic") {
        let a = n(&effects["chromatic"], "amount", 0.0) * 0.06;
        let angle = n(&effects["chromatic"], "angle", 0.0).to_radians();
        filters.push(format!("rgbashift=rh={:.2}:rv={:.2}:bh={:.2}:bv={:.2}", a * angle.cos(), a * angle.sin(), -a * angle.cos(), -a * angle.sin()));
    }
    if enabled(effects, "glow") {
        let radius = (n(&effects["glow"], "radius", 0.0) * 0.04).max(0.1);
        let amount = n(&effects["glow"], "amount", 0.0) / 100.0;
        let threshold = n(&effects["glow"], "threshold", 70.0) / 100.0;
        filters.push(format!("gblur=sigma={radius:.3}:steps=1"));
        filters.push(format!("eq=brightness={:.4}:contrast={:.4}", amount * (0.2 - threshold * 0.08), 1.0 + threshold * 0.12));
    }
    if enabled(effects, "glitch") {
        let speed = n(&effects["glitch"], "speed", 50.0) / 50.0;
        let a = n(&effects["glitch"], "amount", 0.0) * 0.1 * (0.4 + n(&effects["glitch"], "blocks", 25.0) / 40.0) * speed.max(0.1);
        filters.push(format!("rgbashift=rh={a:.2}:bh={:.2}", -a));
        filters.push(format!("noise=alls={:.2}:allf=t", a * 2.0));
    }
    if enabled(effects, "pixelate") {
        let size = (n(&effects["pixelate"], "size", 12.0) * n(&effects["pixelate"], "blend", 100.0) / 100.0).clamp(1.0, 80.0);
        filters.push(format!("scale={}:{}:flags=neighbor,scale={width}:{height}:flags=neighbor", (width as f64 / size).max(1.0) as u32, (height as f64 / size).max(1.0) as u32));
    }
    if enabled(effects, "sharpen") {
        let sharpen = &effects["sharpen"];
        let radius = (n(sharpen, "radius", 5.0).round() as i32).clamp(3, 23) | 1;
        let threshold = 1.0 - n(sharpen, "threshold", 10.0).clamp(0.0, 100.0) / 125.0;
        let amount = n(sharpen, "amount", 0.0) / 100.0 * threshold;
        filters.push(format!("unsharp={radius}:{radius}:{amount:.3}:5:5:0"));
    }
    for key in ["lens", "fisheye"] {
        if enabled(effects, key) {
            let amount = n(&effects[key], "amount", 0.0) / 100.0 * 0.22 * n(&effects[key], "blend", 100.0) / 100.0;
            filters.push(format!("lenscorrection=cx=0.5:cy=0.5:k1={amount:.4}:k2=0"));
            let zoom = (n(&effects[key], "scale", 100.0) / 100.0).clamp(0.5, 1.5);
            if (zoom - 1.0).abs() > 0.001 { filters.push(format!("scale=iw*{zoom:.4}:ih*{zoom:.4}")); }
        }
    }
    if enabled(effects, "scanlines") { filters.push(format!("drawgrid=width=iw:height={}:thickness=1:color=black@{:.3}", n(&effects["scanlines"], "spacing", 3.0).max(1.0), (n(&effects["scanlines"], "amount", 0.0) / 100.0).clamp(0.0, 1.0))); }
    if enabled(effects, "shake") {
        // Motion is added to the overlay coordinates below, based on this enabled value.
    }
    if enabled(effects, "filmBurn") {
        let burn = &effects["filmBurn"];
        let amount = n(burn, "amount", 0.0) / 100.0 * 0.34;
        let warmth = n(burn, "warmth", 0.0) / 100.0;
        let spread = (0.12 + n(burn, "edge", 38.0) / 100.0 * 0.72).clamp(0.12, 0.84);
        let flicker = n(burn, "flicker", 12.0) / 100.0 * 0.45;
        let edge = format!("(max(0,(abs(X-W/2)/(W/2)-{:.4})/{spread:.4})*(1+sin(T*8*PI)*{flicker:.4}))", 1.0 - spread);
        filters.push(format!("geq=r='min(255,r(X,Y)+255*{:.4}*{edge})':g='min(255,g(X,Y)+255*{:.4}*{edge})':b='min(255,b(X,Y)+255*{:.4}*{edge})'", amount * warmth, amount * 0.28, amount * 0.06));
    }
    if enabled(effects, "lightLeak") {
        let leak = &effects["lightLeak"];
        let amount = n(leak, "amount", 0.0) / 100.0 * 0.5;
        let warmth = n(leak, "warmth", 70.0) / 100.0;
        let position = n(leak, "position", 35.0) / 100.0;
        let radius = (0.14 + warmth * 0.38).clamp(0.14, 0.52);
        let hotspot = format!("max(0,1-abs(X/W-{position:.4})/{radius:.4})");
        filters.push(format!("geq=r='min(255,r(X,Y)+255*{:.4}*{hotspot})':g='min(255,g(X,Y)+255*{:.4}*{hotspot})':b='min(255,b(X,Y)+255*{:.4}*{hotspot})'", amount, amount * (0.25 + warmth * 0.2), amount * (1.0 - warmth) * 0.25));
    }
    if enabled(effects, "halftone") {
        let halftone = &effects["halftone"];
        let size = n(halftone, "size", 5.0).clamp(1.0, 24.0) * 2.0;
        let angle = n(halftone, "angle", 45.0).to_radians();
        let contrast = n(halftone, "contrast", 48.0).clamp(0.0, 100.0) / 100.0;
        let px = format!("mod(X*{:.5}+Y*{:.5}+{:.3},{size:.3})-{:.3}", angle.cos(), angle.sin(), size / 2.0, size / 2.0);
        let py = format!("mod(-X*{:.5}+Y*{:.5}+{:.3},{size:.3})-{:.3}", angle.sin(), angle.cos(), size / 2.0, size / 2.0);
        let radius = format!("(1-lum(X,Y)/255)*{:.3}*{contrast:.3}", size / 2.0);
        filters.push(format!("geq=r='if(lt(hypot({px},{py}),{radius}),0,r(X,Y))':g='if(lt(hypot({px},{py}),{radius}),0,g(X,Y))':b='if(lt(hypot({px},{py}),{radius}),0,b(X,Y))'"));
    }
    if enabled(effects, "posterize") {
        let posterize = &effects["posterize"];
        let blend = n(posterize, "blend", 100.0).clamp(0.0, 100.0) / 100.0;
        let target = n(posterize, "levels", 6.0).clamp(2.0, 16.0);
        if blend > 0.0 { filters.push(format!("posterize=levels={:.0}", (16.0 - (16.0 - target) * blend).clamp(2.0, 16.0))); }
    }

    if let Some((kind, transition_duration)) = transition_in {
        let d = transition_duration.clamp(0.05, length.max(0.05));
        if kind == "dip" {
            filters.push(format!("fade=t=in:st={start:.5}:d={:.5}", (d / 2.0).max(0.025)));
        } else if kind == "wipe" {
            filters.push(format!("geq=a='alpha(X,Y)*if(lte(X/W,min(max((T-{start:.5})/{d:.5},0),1)),1,0)'"));
        } else if kind == "iris" {
            filters.push(format!("geq=a='alpha(X,Y)*if(lte(hypot(X-W/2,Y-H/2),min(W,H)*0.5*min(max((T-{start:.5})/{d:.5},0),1)),1,0)'"));
        } else {
            filters.push(format!("fade=t=in:st={start:.5}:d={d:.5}:alpha=1"));
        }
    }
    if let Some((kind, transition_duration)) = transition_out {
        let d = transition_duration.clamp(0.05, length.max(0.05));
        if kind == "dip" { filters.push(format!("fade=t=out:st={:.5}:d={:.5}", start + length - d / 2.0, (d / 2.0).max(0.025))); }
        filters.push(format!("tpad=stop_mode=clone:stop_duration={d:.5}"));
    }

    let distort = &transform["distort"];
    let tl = n(distort, "tl", 0.0) * width as f64 / 1920.0;
    let tr = n(distort, "tr", 0.0) * width as f64 / 1920.0;
    let br = n(distort, "br", 0.0) * width as f64 / 1920.0;
    let bl = n(distort, "bl", 0.0) * width as f64 / 1920.0;
    if tl.abs() + tr.abs() + br.abs() + bl.abs() > 0.01 {
        filters.push(format!("perspective=x0={tl:.3}:y0={tl:.3}:x1={:.3}:y1={tr:.3}:x2={:.3}:y2={:.3}:x3={bl:.3}:y3={:.3}:sense=source:eval=frame", width as f64 - tr, width as f64 - br, height as f64 - br, height as f64 - bl));
    }

    let scale_w = (n(&transform["scale"], "w", 100.0) / 100.0).clamp(0.01, 10.0);
    let scale_h = (n(&transform["scale"], "h", 100.0) / 100.0).clamp(0.01, 10.0);
    let scale_w_key = keyframe_number_expr(clip, &["transform", "scale", "w"], 100.0, keyframe_start, "t");
    let scale_h_key = keyframe_number_expr(clip, &["transform", "scale", "h"], 100.0, keyframe_start, "t");
    if scale_w_key.is_some() || scale_h_key.is_some() {
        let w = scale_w_key.unwrap_or_else(|| format!("{:.8}", n(&transform["scale"], "w", 100.0)));
        let h = scale_h_key.unwrap_or_else(|| format!("{:.8}", n(&transform["scale"], "h", 100.0)));
        filters.push(format!("scale=w='iw*min(max(({w})/100,0.01),10)':h='ih*min(max(({h})/100,0.01),10)':eval=frame"));
    } else if (scale_w - 1.0).abs() > 0.0001 || (scale_h - 1.0).abs() > 0.0001 {
        filters.push(format!("scale=iw*{scale_w:.5}:ih*{scale_h:.5}"));
    }
    let rotation = n(transform, "rotation", 0.0).to_radians();
    if let Some(rotation_expr) = keyframe_number_expr(clip, &["transform", "rotation"], 0.0, keyframe_start, "t") {
        let diagonal = (f64::from(width).hypot(f64::from(height))).ceil() as u32;
        filters.push(format!("rotate='({rotation_expr})*PI/180':ow={diagonal}:oh={diagonal}:c=black@0"));
    } else if rotation.abs() > 0.0001 {
        filters.push(format!("rotate={rotation:.6}:ow=rotw(iw):oh=roth(ih):c=black@0"));
    }
    let opacity = (n(transform, "opacity", 100.0) / 100.0).clamp(0.0, 1.0);
    if let Some(opacity_expr) = keyframe_number_expr(clip, &["transform", "opacity"], 100.0, keyframe_start, "T") {
        filters.push(format!("geq=a='alpha(X,Y)*min(max(({opacity_expr})/100,0),1)'"));
    } else if opacity < 0.999 { filters.push(format!("colorchannelmixer=aa={opacity:.4}")); }
    let x = n(&transform["position"], "x", 0.0) * width as f64 / 1920.0;
    let y = n(&transform["position"], "y", 0.0) * height as f64 / 1080.0;
    let x_expr = keyframe_number_expr(clip, &["transform", "position", "x"], 0.0, keyframe_start, "t")
        .map(|expr| format!("({expr})*{width}/1920")).unwrap_or_else(|| format!("{x:.3}"));
    let y_expr = keyframe_number_expr(clip, &["transform", "position", "y"], 0.0, keyframe_start, "t")
        .map(|expr| format!("({expr})*{height}/1080")).unwrap_or_else(|| format!("{y:.3}"));
    let (shake_x, shake_y) = if enabled(effects, "shake") {
        let amount = n(&effects["shake"], "amount", 0.0) * width as f64 / 1920.0 * 0.18;
        let freq = (n(&effects["shake"], "frequency", 8.0) * (1.0 - n(&effects["shake"], "smoothness", 35.0) / 120.0)).max(0.5);
        (format!("+sin(2*PI*t*{freq:.3})*{amount:.3}"), format!("+cos(2*PI*t*{freq:.3})*{amount:.3}"))
    } else { (String::new(), String::new()) };
    let overlay_x = format!("(main_w-overlay_w)/2+({x_expr}){shake_x}");
    let overlay_y = format!("(main_h-overlay_h)/2+({y_expr}){shake_y}");
    (filters.join(","), overlay_x, overlay_y)
}

fn video_filters(clip: &Value, width: u32, height: u32, start: f64, length: f64, transition_in: Option<(&str, f64)>, transition_out: Option<(&str, f64)>) -> (String, String, String) {
    video_filters_at(clip, width, height, start, length, start, transition_in, transition_out)
}

pub fn preview_video_filter(clip: &Value, width: u32, height: u32) -> String {
    let duration = n(clip, "dur", 1.0).max(0.05);
    video_filters(clip, width, height, 0.0, duration, None, None).0
}

/// Build a complete single-clip preview graph, including the editor canvas,
/// clip transforms, and the mirror effect. The input is `[0:v]` and the final
/// composited frame is exposed as `[out]` for the native preview worker.
pub fn preview_video_graph(clip: &Value, width: u32, height: u32, timeline_time: f64) -> String {
    let clip_start = n(clip, "start", 0.0).max(0.0);
    let duration = n(clip, "dur", 1.0).max(0.05);
    let local_time = (timeline_time - clip_start).clamp(0.0, duration);
    let preview_time = clip_start + local_time;
    let remaining = (duration - local_time).max(0.05);
    // The media input is already seeked to the current source frame. Rebase its
    // timestamps to the playhead for animated FFmpeg filters, while keyframe
    // expressions remain anchored to the original clip start.
    let (filters, x, y) = video_filters_at(clip, width, height, preview_time, remaining, clip_start, None, None);
    let effects = &clip["props"]["effects"];
    let mirror = &effects["mirror"];
    let mirror_mix = if enabled(effects, "mirror") {
        (n(mirror, "blend", 100.0) / 100.0).clamp(0.0, 1.0)
    } else {
        0.0
    };

    let mut graph = vec![format!("[0:v]{filters}[preview_raw]")];
    if mirror_mix > 0.0 {
        graph.push("[preview_raw]split=2[preview_normal][preview_mirrored]".to_owned());
        graph.push(format!(
            "[preview_mirrored]{}[preview_flipped]",
            if mirror["axis"].as_str() == Some("vertical") { "vflip" } else { "hflip" }
        ));
        graph.push(format!(
            "[preview_normal][preview_flipped]blend=all_expr='A*{:.4}+B*{mirror_mix:.4}'[preview_clip]",
            1.0 - mirror_mix
        ));
    } else {
        graph.push("[preview_raw]null[preview_clip]".to_owned());
    }
    graph.push(format!("color=c=black:s={width}x{height}:r=30,setpts=PTS+{preview_time:.8}/TB[preview_canvas]"));
    graph.push(format!(
        "[preview_canvas][preview_clip]overlay=x='{x}':y='{y}':shortest=1:eof_action=pass:format=auto[out]"
    ));
    graph.join(";")
}

fn audio_filters(clip: &Value, length: f64, total: f64) -> String {
    let props = &clip["props"];
    let audio = &props["audio"];
    let effects = &props["effects"];
    let mut filters = vec![format!("atrim=start={:.5}:duration={length:.5}", n(clip, "sourceOffset", 0.0).max(0.0)), "asetpts=PTS-STARTPTS".into()];
    let volume = (n(audio, "volume", 100.0) / 100.0).clamp(0.0, 2.0);
    if let Some(volume_expr) = keyframe_number_expr(clip, &["audio", "volume"], 100.0, 0.0, "t") {
        filters.push(format!("volume='min(max(({volume_expr})/100,0),2)':eval=frame"));
    } else if (volume - 1.0).abs() > 0.0001 { filters.push(format!("volume={volume:.4}")); }
    let pan = (n(audio, "pan", 0.0) / 100.0).clamp(-1.0, 1.0);
    if pan.abs() > 0.001 { filters.push(format!("stereotools=balance_out={pan:.4}")); }
    let bass = n(audio, "bass", 0.0); let treble = n(audio, "treble", 0.0);
    if bass.abs() > 0.001 { filters.push(format!("equalizer=f=100:t=q:w=1:g={bass:.3}")); }
    if treble.abs() > 0.001 { filters.push(format!("equalizer=f=4000:t=q:w=1:g={treble:.3}")); }
    if enabled(effects, "eq") {
        for (key, freq) in [("low", 100), ("mid", 1000), ("high", 8000)] {
            let gain = n(&effects["eq"], key, 0.0);
            if gain.abs() > 0.001 { filters.push(format!("equalizer=f={freq}:t=q:w=1:g={gain:.3}")); }
        }
    }
    if enabled(effects, "reverb") {
        let wet = (n(&effects["reverb"], "wet", 0.0) / 100.0).clamp(0.0, 1.0);
        let delay = (80.0 + n(&effects["reverb"], "room", 0.0) * 3.0).round() as u32;
        let damping = 1.0 - n(&effects["reverb"], "damp", 50.0).clamp(0.0, 100.0) / 125.0;
        filters.push(format!("aecho=1.0:{:.3}:{delay}|{}:{:.3}|{:.3}", 1.0 - wet * 0.45, delay * 2, wet * 0.35 * damping, wet * 0.18 * damping));
    }
    if enabled(effects, "delay") {
        let delay = n(&effects["delay"], "time", 320.0).clamp(1.0, 2000.0).round() as u32;
        let feedback = (n(&effects["delay"], "feedback", 0.0) / 100.0).clamp(0.0, 0.95);
        let mix = (n(&effects["delay"], "mix", 0.0) / 100.0).clamp(0.0, 1.0);
        filters.push(format!("aecho=1.0:{:.3}:{delay}|{}:{feedback:.3}|{:.3}", 1.0 - mix * 0.5, delay * 2, feedback * 0.5));
    }
    if enabled(effects, "compressor") { filters.push(format!("acompressor=threshold={:.3}dB:ratio={:.3}:attack={:.3}:release={:.3}", n(&effects["compressor"], "threshold", -18.0), n(&effects["compressor"], "ratio", 4.0), n(&effects["compressor"], "attack", 12.0), n(&effects["compressor"], "release", 140.0))); }
    if enabled(effects, "limiter") { filters.push(format!("alimiter=limit={:.5}:release={:.3}", 10_f64.powf(n(&effects["limiter"], "ceiling", -1.0) / 20.0), n(&effects["limiter"], "release", 80.0))); }
    if enabled(effects, "denoise") {
        let reduction = n(&effects["denoise"], "reduction", 0.0);
        let sensitivity = n(&effects["denoise"], "sensitivity", 50.0).clamp(0.0, 100.0);
        filters.push(format!("afftdn=nr={:.1}:nf={:.1}", (reduction * 0.24).clamp(0.0, 24.0), -50.0 + sensitivity * 0.3));
    }
    if audio["fadeIn"].as_bool().unwrap_or(false) { filters.push(format!("afade=t=in:st=0:d={:.4}", n(audio, "fadeInDuration", 1.0).clamp(0.0, length))); }
    if audio["fadeOut"].as_bool().unwrap_or(false) { let d = n(audio, "fadeOutDuration", 1.0).clamp(0.0, length); filters.push(format!("afade=t=out:st={:.4}:d={d:.4}", (length - d).max(0.0))); }
    let start = n(clip, "start", 0.0).max(0.0);
    filters.push(format!("adelay={}:all=1", (start * 1000.0).round() as u64));
    let _ = total;
    filters.join(",")
}

pub fn render_timeline(process: SharedExport, clips: Vec<Value>, settings: Value, time: f64, output: PathBuf, progress: std::sync::mpsc::Sender<f64>) -> Result<String, String> {
    let width = n(&settings["size"], "width", 1920.0).clamp(16.0, 7680.0) as u32;
    let height = n(&settings["size"], "height", 1080.0).clamp(16.0, 4320.0) as u32;
    let fps = n(&settings["video"], "fps", 30.0).clamp(1.0, 120.0) as u32;
    let mode = settings["mode"].as_str().unwrap_or("video");
    let format = if mode == "image" { settings["image"]["format"].as_str().unwrap_or("png") } else { settings["video"]["format"].as_str().unwrap_or("mp4") };
    let include_audio = mode == "video" && format != "gif" && settings["video"]["audio"].as_bool().unwrap_or(true);
    let duration = clips.iter().filter_map(|clip| Some(n(clip, "start", 0.0) + n(clip, "dur", 0.0))).fold(0.0_f64, f64::max);
    if duration <= 0.0 { return Err("Add media to the timeline before exporting".into()); }
    let mut visual: Vec<_> = clips.iter().filter(|clip| clip["track"].as_str().map(|track| track.starts_with('v')).unwrap_or(false) && clip["path"].as_str().is_some()).collect();
    let titles: Vec<_> = clips.iter().filter(|clip| clip["theme"].as_str() == Some("title")).collect();
    visual.sort_by(|a,b| a["track"].as_str().cmp(&b["track"].as_str()).then_with(|| n(a, "start", 0.0).partial_cmp(&n(b, "start", 0.0)).unwrap_or(std::cmp::Ordering::Equal)));
    let audio: Vec<_> = if include_audio { clips.iter().filter(|clip| {
        let Some(track) = clip["track"].as_str() else { return false; };
        if clip["path"].as_str().is_none() { return false; }
        if track.starts_with('a') { return true; }
        if !track.starts_with('v') || clip["hasAudio"].as_bool() != Some(true)
            || clip["audioDetached"].as_bool() == Some(true) || !clip["linkedAudioId"].is_null() { return false; }
        let path = clip["path"].as_str().unwrap_or_default();
        let start = n(clip, "start", 0.0);
        !clips.iter().any(|candidate| candidate["track"].as_str().map(|lane| lane.starts_with('a')).unwrap_or(false)
            && candidate["path"].as_str() == Some(path)
            && (n(candidate, "start", 0.0) - start).abs() < 1.0 / 30.0)
    }).collect() } else { Vec::new() };
    let mut transition_ends_by_track: HashMap<String, Vec<(f64, usize, &Value)>> = HashMap::new();
    let mut visual_starts_by_track: HashMap<String, Vec<f64>> = HashMap::new();
    for (order, clip) in clips.iter().enumerate() {
        let Some(track) = clip["track"].as_str() else { continue; };
        if clip["trans"].is_object() {
            transition_ends_by_track.entry(track.to_owned()).or_default()
                .push((n(clip, "start", 0.0) + n(clip, "dur", 0.0), order, clip));
        }
        if clip["path"].as_str().is_some() && track.starts_with('v') {
            visual_starts_by_track.entry(track.to_owned()).or_default().push(n(clip, "start", 0.0));
        }
    }
    for neighbors in transition_ends_by_track.values_mut() {
        neighbors.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.1.cmp(&b.1)));
    }
    for starts in visual_starts_by_track.values_mut() { starts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal)); }
    if visual.is_empty() && titles.is_empty() { return Err("Add imported media or a title clip to the timeline before exporting".into()); }
    if let Some(clip) = visual.iter().chain(audio.iter()).find(|clip| !PathBuf::from(clip["path"].as_str().unwrap_or_default()).is_file()) {
        return Err(format!("Media file is missing: {}", clip["path"].as_str().unwrap_or("(unknown)")));
    }
    let transparent_background = mode == "image" && settings["image"]["transparent"].as_bool().unwrap_or(false);
    let background = if transparent_background { "black@0" } else { "black" };
    let mut args: Vec<String> = vec!["-y".into(), "-hide_banner".into(), "-loglevel".into(), "error".into(), "-progress".into(), "pipe:1".into(), "-nostats".into(), "-f".into(), "lavfi".into(), "-i".into(), format!("color=c={background}:s={width}x{height}:r={fps}:d={duration:.5}")];
    let mut input_index = 1_usize;
    let mut visual_inputs = Vec::new();
    for clip in &visual {
        let path = clip["path"].as_str().unwrap();
        let length = n(clip, "dur", 0.0).max(0.05);
        if clip["mediaKind"].as_str() == Some("image") { args.extend(["-loop".into(), "1".into(), "-t".into(), format!("{:.5}", length + n(clip, "sourceOffset", 0.0).max(0.0))]); }
        args.extend(["-i".into(), path.to_owned()]);
        visual_inputs.push(input_index); input_index += 1;
    }
    let mut audio_inputs = Vec::new();
    for clip in &audio { args.extend(["-i".into(), clip["path"].as_str().unwrap().to_owned()]); audio_inputs.push(input_index); input_index += 1; }

    let mut graph = vec!["[0:v]format=rgba[base0]".to_owned()];
    let mut base = "base0".to_owned();
    for (index, clip) in visual.iter().enumerate() {
        let input = visual_inputs[index];
        let length = n(clip, "dur", 0.0).max(0.05);
        let start = n(clip, "start", 0.0).max(0.0);
        let track = clip["track"].as_str().unwrap_or_default();
        let previous_transition = transition_ends_by_track.get(track).and_then(|neighbors| {
            let first = neighbors.partition_point(|(end, _, _)| *end < start - 0.05);
            neighbors[first..].iter().take_while(|(end, _, _)| *end < start + 0.05)
                .filter(|(end, _, _)| (*end - start).abs() < 0.05)
                .min_by_key(|(_, order, _)| *order)
                .map(|(_, _, other)| (other["trans"]["type"].as_str().unwrap_or("dissolve"), n(&other["trans"], "dur", 1.0)))
        });
        let end = start + length;
        let has_next = visual_starts_by_track.get(track).map(|starts| {
            let first = starts.partition_point(|next_start| *next_start < end - 0.05);
            starts[first..].iter().take_while(|next_start| **next_start < end + 0.05)
                .any(|next_start| (*next_start - end).abs() < 0.05)
        }).unwrap_or(false);
        let next_transition = if has_next {
            let trans = &clip["trans"];
            trans.is_object().then(|| (trans["type"].as_str().unwrap_or("dissolve"), n(trans, "dur", 1.0)))
        } else { None };
        let (filters, x, y) = video_filters(clip, width, height, start, length, previous_transition, next_transition);
        let raw = format!("visualraw{index}");
        let out = format!("visual{index}");
        graph.push(format!("[{input}:v]{filters}[{raw}]"));
        let effects = &clip["props"]["effects"];
        let mirror = &effects["mirror"];
        let mirror_mix = if enabled(effects, "mirror") { (n(mirror, "blend", 100.0) / 100.0).clamp(0.0, 1.0) } else { 0.0 };
        if mirror_mix > 0.0 {
            let normal = format!("mirror_normal{index}");
            let mirrored = format!("mirror_flip{index}");
            graph.push(format!("[{raw}]split=2[{normal}][{mirrored}]"));
            graph.push(format!("[{mirrored}]{}[mirror_flipped{index}]", if mirror["axis"].as_str() == Some("vertical") { "vflip" } else { "hflip" }));
            graph.push(format!("[{normal}][mirror_flipped{index}]blend=all_expr='A*{:.4}+B*{mirror_mix:.4}'[{out}]", 1.0 - mirror_mix));
        } else { graph.push(format!("[{raw}]null[{out}]")); }
        let next = format!("base{}", index + 1);
        let overlay_end = end + next_transition.map(|(_, d)| d).unwrap_or(0.0);
        graph.push(format!("[{base}][{out}]overlay=x='{x}':y='{y}':enable='between(t,{start:.5},{overlay_end:.5})':eof_action=pass:shortest=0[{next}]"));
        base = next;
    }
    let mut temp_files: Option<ExportTempFiles> = None;
    if !titles.is_empty() || clips.iter().any(|clip| clip["props"]["text"]["caption"].as_str().map(|text| !text.is_empty()).unwrap_or(false)) {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
        let temp_root = std::env::temp_dir().join(format!("pluto-export-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&temp_root).map_err(|error| format!("Could not prepare text rendering: {error}"))?;
        temp_files = Some(ExportTempFiles(temp_root.clone()));
        let font = std::env::var_os("WINDIR").map(|root| PathBuf::from(root).join("Fonts").join("segoeui.ttf")).filter(|path| path.is_file());
        let font_option = font.as_ref().map(|path| format!("fontfile='{}'", ffmpeg_filter_path(path))).unwrap_or_else(|| "font='Arial'".into());
        let mut text_index = 0_usize;
        for clip in &titles {
            let start = n(clip, "start", 0.0).max(0.0);
            let end = start + n(clip, "dur", duration - start).max(0.05);
            let text = &clip["props"]["text"];
            let transform = &clip["props"]["transform"];
            let opacity = (n(transform, "opacity", 100.0) / 100.0).clamp(0.0, 1.0);
            let position_x = n(&transform["position"], "x", 0.0) * width as f64 / 1920.0;
            let position_y = n(&transform["position"], "y", 0.0) * height as f64 / 1080.0;
            let size = (48.0 * n(text, "size", 100.0) / 100.0 * n(&transform["scale"], "h", 100.0) / 100.0 * height as f64 / 1080.0).clamp(8.0, 500.0);
            let align = text["align"].as_str().unwrap_or("center");
            let x = match align { "left" => format!("{position_x:.2}"), "right" => format!("w-text_w+{position_x:.2}"), _ => format!("(w-text_w)/2+{position_x:.2}") };
            for (field, y) in [("title", format!("(h-text_h)/2+{position_y:.2}")), ("sub", format!("(h-text_h)/2+{:.2}", position_y + size * 0.78))] {
                let value = text[field].as_str().unwrap_or("");
                if value.is_empty() { continue; }
                let text_path = temp_root.join(format!("text-{text_index}.txt")); text_index += 1;
                fs::write(&text_path, value).map_err(|error| format!("Could not prepare title text: {error}"))?;
                let next = format!("text{}", text_index);
                let font_size = if field == "sub" { size * 0.36 } else { size };
                graph.push(format!("[{base}]drawtext={font_option}:textfile='{}':expansion=none:fontsize={font_size:.2}:fontcolor=white@{opacity:.3}:borderw=2:bordercolor=black@0.55:x='{x}':y='{y}':enable='between(t,{start:.5},{end:.5})'[{next}]", ffmpeg_filter_path(&text_path)));
                base = next;
            }
        }
        for clip in clips.iter().filter(|clip| clip["props"]["text"]["caption"].as_str().map(|text| !text.is_empty()).unwrap_or(false)) {
            let value = clip["props"]["text"]["caption"].as_str().unwrap_or("");
            let text_path = temp_root.join(format!("text-{text_index}.txt")); text_index += 1;
            fs::write(&text_path, value).map_err(|error| format!("Could not prepare caption text: {error}"))?;
            let start = n(clip, "start", 0.0).max(0.0); let end = start + n(clip, "dur", 0.0).max(0.05);
            let next = format!("text{}", text_index);
            let font_size = (32.0 * height as f64 / 1080.0).clamp(8.0, 200.0);
            graph.push(format!("[{base}]drawtext={font_option}:textfile='{}':expansion=none:fontsize={font_size:.2}:fontcolor=white:borderw=2:bordercolor=black@0.6:x='(w-text_w)/2':y='h-text_h-48':enable='between(t,{start:.5},{end:.5})'[{next}]", ffmpeg_filter_path(&text_path)));
            base = next;
        }
    }
    for (index, clip) in audio.iter().enumerate() {
        let input = audio_inputs[index];
        let length = n(clip, "dur", 0.0).max(0.05);
        graph.push(format!("[{input}:a]{}[audio{index}]", audio_filters(clip, length, duration)));
    }
    if !audio.is_empty() {
        let labels = (0..audio.len()).map(|i| format!("[audio{i}]")).collect::<String>();
        graph.push(format!("{labels}amix=inputs={}:duration=longest:normalize=0,apad=whole_dur={duration:.5}[aout]", audio.len()));
    }
    args.extend(["-filter_complex".into(), graph.join(";")]);
    args.extend(["-map".into(), format!("[{base}]")]);
    if mode == "image" {
        args.extend(["-ss".into(), format!("{:.5}", time.clamp(0.0, duration)) , "-frames:v".into(), "1".into()]);
        let quality = n(&settings["image"], "quality", 90.0).clamp(1.0, 100.0);
        match format {
            "jpg" => args.extend(["-c:v".into(), "mjpeg".into(), "-q:v".into(), format!("{}", (31.0 - quality * 0.3).round().clamp(2.0, 31.0) as u32)]),
            "webp" => args.extend(["-c:v".into(), "libwebp".into(), "-quality".into(), format!("{quality:.0}")]),
            "avif" => args.extend(["-c:v".into(), "libaom-av1".into(), "-crf".into(), format!("{:.0}", 63.0 - quality * 0.6), "-still-picture".into(), "1".into()]),
            _ => args.extend(["-c:v".into(), "png".into()]),
        }
    } else {
        args.extend(["-r".into(), fps.to_string()]);
        match format {
            "webm" => args.extend(["-c:v".into(), "libvpx-vp9".into(), "-b:v".into(), "0".into(), "-crf".into(), match settings["video"]["quality"].as_str().unwrap_or("high") { "low" => "40", "medium" => "32", "max" => "18", _ => "26" }.into()]),
            "gif" => args.extend(["-c:v".into(), "gif".into(), "-loop".into(), if settings["video"]["loop"].as_bool().unwrap_or(true) { "0" } else { "-1" }.into()]),
            _ => args.extend(["-c:v".into(), "libx264".into(), "-pix_fmt".into(), "yuv420p".into(), "-crf".into(), match settings["video"]["quality"].as_str().unwrap_or("high") { "low" => "28", "medium" => "23", "max" => "14", _ => "18" }.into()]),
        }
        if !audio.is_empty() {
            args.extend(["-map".into(), "[aout]".into()]);
            if format == "webm" { args.extend(["-c:a".into(), "libopus".into()]); }
            else if format != "gif" { args.extend(["-c:a".into(), "aac".into()]); }
        }
        if format == "mp4" { args.extend(["-movflags".into(), "+faststart".into()]); }
    }
    if mode == "image" {
        if settings["image"]["metadata"].as_bool().unwrap_or(true) {
            if let Some(name) = settings["output"]["name"].as_str() { args.extend(["-metadata".into(), format!("title={name}")]); }
        } else { args.extend(["-map_metadata".into(), "-1".into()]); }
        if settings["image"]["colorSpace"].as_str() == Some("p3") {
            args.extend(["-color_primaries".into(), "smpte432".into(), "-color_trc".into(), "iec61966-2-1".into(), "-colorspace".into(), "bt709".into()]);
        }
    }
    args.push(output.to_string_lossy().into_owned());
    let mut current = process.lock().map_err(|_| "Export process lock failed".to_owned())?;
    if current.is_some() { return Err("Another export is already running".into()); }
    let mut child = multimedia_command("ffmpeg")
        .args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|error| format!("Could not start FFmpeg. Install FFmpeg or set PLUTO_FFMPEG_DIR: {error}"))?;
    if let Some(stdout) = child.stdout.take() {
        let progress_tx = progress.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().flatten() {
                if let Some(value) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
                    let _ = progress_tx.send((value / 1_000_000.0 / duration).clamp(0.0, 1.0));
                } else if line == "progress=end" { let _ = progress_tx.send(1.0); }
            }
        });
    }
    let stderr_reader = child.stderr.take().map(|stderr| thread::spawn(move || {
        let mut message = String::new();
        let _ = BufReader::new(stderr).read_to_string(&mut message);
        message
    }));
    *current = Some(child);
    drop(current);
    loop {
        let status = {
            let mut guard = process.lock().map_err(|_| "Export process lock failed".to_owned())?;
            let result = guard.as_mut().ok_or_else(|| "Export canceled".to_owned())?.try_wait();
            match result {
                Ok(status) => status,
                Err(error) => {
                    if let Some(mut child) = guard.take() { let _ = child.kill(); let _ = child.wait(); }
                    let _ = fs::remove_file(&output);
                    return Err(format!("Could not read FFmpeg export status: {error}"));
                }
            }
        };
        if let Some(status) = status {
            let child = process.lock().map_err(|_| "Export process lock failed".to_owned())?.take().ok_or_else(|| "Export canceled".to_owned())?;
            let _ = child.wait_with_output();
            if status.success() { return Ok(output.to_string_lossy().into_owned()); }
            let message = stderr_reader.and_then(|reader| reader.join().ok()).unwrap_or_default();
            let _ = fs::remove_file(&output);
            let message = message.trim().to_owned();
            return Err(if message.is_empty() { "FFmpeg export failed or was canceled".into() } else { message });
        }
        thread::sleep(Duration::from_millis(80));
    }
}


#[cfg(test)]
mod preview_tests {
    use super::*;

    fn sample_clip() -> Value {
        serde_json::json!({
            "start": 4.0,
            "dur": 6.0,
            "sourceOffset": 0.0,
            "props": {
                "transform": {
                    "scale": { "w": 100.0, "h": 100.0 },
                    "position": { "x": 0.0, "y": 0.0 },
                    "rotation": 0.0,
                    "opacity": 100.0
                },
                "colors": {},
                "effects": {}
            }
        })
    }

    #[test]
    fn preview_graph_composites_the_seeked_frame_at_playhead_time() {
        let graph = preview_video_graph(&sample_clip(), 640, 360, 7.0);
        assert!(graph.contains("trim=start=0.00000:duration=3.00000"));
        assert!(graph.contains("setpts=PTS-STARTPTS+7.00000/TB"));
        assert!(graph.contains("setpts=PTS+7.00000000/TB"));
        assert!(graph.contains("overlay="));
        assert!(graph.contains("[out]"));
    }

    #[test]
    fn preview_keyframes_remain_relative_to_the_original_clip_start() {
        let mut clip = sample_clip();
        clip["keyframes"] = serde_json::json!([
            {
                "time": 1.0,
                "props": { "transform": { "position": { "x": 120.0 } } }
            }
        ]);
        let graph = preview_video_graph(&clip, 640, 360, 5.5);
        assert!(graph.contains("if(lt(t,4.00000000),0.00000000"));
        assert!(graph.contains("if(lt(t,5.00000000)"));
    }
}
