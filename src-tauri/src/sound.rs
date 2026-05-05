use std::{fs, path::Path};

use anyhow::{Context, Result};

const COPY_SOUND_BYTES: &[u8] = include_bytes!("../../public/copy.mp3");

pub(crate) fn ensure_copy_sound_file(app_data_dir: &Path) -> Result<std::path::PathBuf> {
    let sound_path = app_data_dir.join("copy.mp3");
    if sound_path.exists() {
        return Ok(sound_path);
    }

    fs::create_dir_all(app_data_dir).context("failed to create app data directory")?;
    fs::write(&sound_path, COPY_SOUND_BYTES).context("failed to write copy sound")?;
    Ok(sound_path)
}

#[cfg(target_os = "windows")]
pub(crate) fn play_copy_sound(app_data_dir: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let sound_path = ensure_copy_sound_file(app_data_dir)?;
    let sound_path = sound_path.to_string_lossy().replace('\'', "''");
    let script = format!(
        r#"$p = New-Object System.Windows.Media.MediaPlayer;
$p.Open([Uri]::new('{sound_path}'));
for ($i = 0; $i -lt 50 -and -not $p.NaturalDuration.HasTimeSpan; $i++) {{
  Start-Sleep -Milliseconds 20;
}}
$p.Volume = 1;
$p.Play();
$duration = 800;
if ($p.NaturalDuration.HasTimeSpan) {{
  $duration = [Math]::Min([int]$p.NaturalDuration.TimeSpan.TotalMilliseconds + 120, 1500);
}}
Start-Sleep -Milliseconds $duration;
$p.Close()"#
    );
    let command = "Add-Type -AssemblyName PresentationCore; ".to_owned() + &script;

    Command::new("powershell")
        .args(["-NoProfile", "-STA", "-WindowStyle", "Hidden", "-Command"])
        .arg(command)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("failed to spawn copy sound player")?;

    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn play_copy_sound(_app_data_dir: &Path) -> Result<()> {
    Ok(())
}
