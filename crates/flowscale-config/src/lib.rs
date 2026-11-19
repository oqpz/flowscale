//! Clean-room `config.ini` parser/writer, app settings, and frame-pacing utilities.

pub mod app_settings;
pub mod ini;
pub mod pacer;
pub mod persistence;

pub use app_settings::{
    get_windows_startup, import_lossless_default_xml, import_lossless_settings_xml,
    matches_profile_path, normalize_scale_factor, set_windows_startup, AppSettings, GameProfile,
    HotkeyConfig, ThemeMode,
};
pub use ini::{CaptureConfig, EngineConfig, FpsCounterConfig, LsfgConfig, RenderingConfig};
pub use pacer::{
    FrametimeTracker, HybridPacer, InterpolationMode, InterpolationScheduler,
    QueueDrainingController, ScheduledStep,
};

#[cfg(test)]
mod tests {
    #[test]
    fn cpu_processing_is_opt_in_and_survives_profile_roundtrip() {
        let old: super::GameProfile = serde_json::from_str("{}").unwrap();
        assert!(!old.cpu_vulkan);
        let cpu = super::GameProfile { cpu_vulkan: true, ..old };
        let encoded = serde_json::to_string(&cpu).unwrap();
        assert!(serde_json::from_str::<super::GameProfile>(&encoded).unwrap().cpu_vulkan);
        assert!(serde_json::from_str::<super::GameProfile>(r#"{"cpu_vulkan":"true"}"#).is_err());
    }
    use super::*;

    #[test]
    fn xml_import_ignores_nonfinite_frame_generation_values() {
        for value in ["NaN", "inf", "-inf"] {
            let xml = format!("<Settings><GameProfiles><Profile><Title>Default</Title><LSFG3Multiplier>{value}</LSFG3Multiplier><LSFG3Target>{value}</LSFG3Target></Profile></GameProfiles></Settings>");
            let (_, profiles) = import_lossless_settings_xml(&xml).unwrap();
            assert_eq!(
                profiles[0].lsfg3_multiplier,
                GameProfile::default().lsfg3_multiplier
            );
            assert_eq!(
                profiles[0].lsfg3_target,
                GameProfile::default().lsfg3_target
            );
        }
    }

    #[test]
    fn frametime_buffer_ini_input_is_bounded() {
        let cfg = EngineConfig::parse("[capture]\nframetime_buffer_size = 18446744073709551615\n");
        assert_eq!(cfg.capture.frametime_buffer_size, 1024);
    }

    #[test]
    fn custom_default_title_never_reserves_default_profile_id() {
        let xml = r#"<Settings><GameProfiles>
          <Profile><Title>Default</Title><Path>C:\\Games\\game.exe</Path></Profile>
          <Profile><Title>Default</Title><Path></Path></Profile>
          <Profile><Title>Default</Title><Path>D:\\Games\\other.exe</Path></Profile>
        </GameProfiles></Settings>"#;
        let (_, profiles) = import_lossless_settings_xml(xml).unwrap();
        assert_ne!(profiles[0].id, "default");
        assert_eq!(profiles[1].id, "default");
        assert_ne!(profiles[2].id, "default");
        assert_ne!(profiles[0].id, profiles[2].id);
    }

    #[test]
    fn missing_and_corrupt_settings_are_distinguished() {
        let root = std::env::temp_dir().join(format!("flowscale-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("settings.json");
        assert!(AppSettings::load_or_default_checked_at(&path).1.is_none());
        std::fs::write(&path, "{ invalid").unwrap();
        let (fallback, error) = AppSettings::load_or_default_checked_at(&path);
        assert!(error.is_some());
        assert_eq!(fallback, AppSettings::default());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ invalid");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn checked_ini_parser_rejects_unrecognized_or_malformed_files() {
        assert!(EngineConfig::parse_checked("random garbage\n").is_err());
        assert!(EngineConfig::parse_checked("[capture\nframetime_buffer_size=15").is_err());
        assert!(EngineConfig::parse_checked("[capture]\nframetime_buffer_size=15\n").is_ok());
    }

    #[test]
    fn parse_and_roundtrip_default_config_ini() {
        let sample = r#"
[rendering]
flush = 1

[capture]
frametime_buffer_size = 15
frametime_buffer_reset_multiplier = 6
queue_draining_momentum = 0.01

[lsfg]
ui_detection_rate = 1
ui_detection_threshold = 0.5
real_timestamp_tolerance = 0.05
base_framerate_threshold = 10

[fps_counter]
scale = 1.0
color = 0xD3D3D3
font = Consolas
offset_x = 0
offset_y = 0
show_captured = 1
"#;
        let parsed = EngineConfig::parse(sample);
        assert!(parsed.rendering.flush);
        assert_eq!(parsed.capture.frametime_buffer_size, 15);
        assert!((parsed.capture.frametime_buffer_reset_multiplier - 6.0).abs() < 1e-9);
        assert!((parsed.capture.queue_draining_momentum - 0.01).abs() < 1e-9);
        assert_eq!(parsed.lsfg.ui_detection_rate, 1);
        assert!((parsed.lsfg.ui_detection_threshold - 0.5).abs() < 1e-9);
        assert!((parsed.lsfg.real_timestamp_tolerance - 0.05).abs() < 1e-9);
        assert!((parsed.lsfg.base_framerate_threshold - 10.0).abs() < 1e-9);
        assert_eq!(parsed.fps_counter.color, 0x00D3_D3D3);
        assert_eq!(parsed.fps_counter.font, "Consolas");
        assert!(parsed.fps_counter.show_captured);

        let serialized = parsed.to_ini_string();
        let reparsed = EngineConfig::parse(&serialized);
        assert_eq!(parsed, reparsed);
    }

    #[test]
    fn frametime_tracker_resets_on_large_spike() {
        let mut tracker = FrametimeTracker::new(15, 6.0);
        for _ in 0..5 {
            tracker.record_delta(1.0 / 60.0);
        }
        assert_eq!(tracker.len(), 5);
        assert!((tracker.estimated_fps() - 60.0).abs() < 0.01);

        // 7x spike should reset the rolling buffer to just the new sample.
        tracker.record_delta((1.0 / 60.0) * 7.0);
        assert_eq!(tracker.len(), 1);
    }

    #[test]
    fn queue_draining_controller_biases_toward_target() {
        let mut ctrl = QueueDrainingController::new(0.5, 1);
        let fast = ctrl.update(3);
        assert!(fast < 1.0);
        let mut ctrl_empty = QueueDrainingController::new(0.5, 1);
        let slow = ctrl_empty.update(0);
        assert!(slow > 1.0);
    }

    #[test]
    fn interpolation_scheduler_fixed_and_adaptive() {
        let mut fixed =
            InterpolationScheduler::new(InterpolationMode::Fixed { multiplier: 3.0 }, 10.0, 0.05);
        let steps = fixed.plan_interval(1.0 / 60.0);
        assert_eq!(steps.len(), 3);
        assert!(steps[0].is_interpolated);
        assert!(steps[1].is_interpolated);
        assert!(!steps[2].is_interpolated);
        assert!((steps[0].alpha - (1.0 / 3.0)).abs() < 1e-5);
        assert!((steps[2].alpha - 1.0).abs() < 1e-5);

        let mut adaptive = InterpolationScheduler::new(
            InterpolationMode::Adaptive { target_fps: 144.0 },
            10.0,
            0.05,
        );
        // 144 / 60 = 2.4 -> sequence over 5 intervals produces 12 frames (2, 2, 3, 2, 3)
        let mut elapsed = 0.0;
        for (interval, count) in [2, 2, 3, 2, 3].into_iter().enumerate() {
            let steps = adaptive.plan_interval(1.0 / 60.0);
            assert_eq!(steps.len(), count);
            for step in steps {
                assert!((step.output_dt_sec - 1.0 / 144.0).abs() < 1e-12);
                elapsed += step.output_dt_sec;
                let phase = elapsed * 60.0 - interval as f64;
                assert!((f64::from(step.alpha) - phase).abs() <= 0.05);
            }
        }
        assert!((elapsed - 5.0 / 60.0).abs() < 1e-12);

        adaptive.plan_interval(1.0 / 60.0);
        let passthrough = adaptive.plan_interval(1.0 / 165.0);
        assert_eq!(passthrough.len(), 1);
        assert!(!passthrough[0].is_interpolated);
        let resumed = adaptive.plan_interval(1.0 / 60.0);
        assert!((resumed[0].alpha - 60.0 / 144.0).abs() < 1e-6);
        let slower = adaptive.plan_interval(1.0 / 48.0);
        assert_eq!(slower.len(), 3);
        assert!(slower
            .iter()
            .all(|step| (step.output_dt_sec - 1.0 / 144.0).abs() < 1e-12));
        assert!((slower[0].alpha - 0.2).abs() < 1e-6);

        // Below base_framerate_threshold (5 FPS < 10 FPS): pass-through real frame only.
        let low = fixed.plan_interval(0.2);
        assert_eq!(low.len(), 1);
        assert!(!low[0].is_interpolated);
    }

    #[test]
    fn hybrid_pacer_waits_accurately() {
        let pacer = HybridPacer::new().expect("HybridPacer::new failed");
        let start = pacer.now_sec();
        pacer.wait_until_sec(start + 0.005);
        let elapsed = pacer.now_sec() - start;
        assert!(elapsed >= 0.0045, "elapsed too short: {elapsed}");
    }

    #[test]
    fn normalize_scale_factor_rules() {
        use flowscale_abi::ScalingType;

        assert_eq!(normalize_scale_factor(ScalingType::Integer, 0.5), 1.0);
        assert_eq!(normalize_scale_factor(ScalingType::Integer, 2.6), 3.0);
        assert_eq!(normalize_scale_factor(ScalingType::Integer, 12.0), 10.0);

        assert!((normalize_scale_factor(ScalingType::Xbr, 1.3) - 1.2).abs() < 1e-6);
        assert!((normalize_scale_factor(ScalingType::Xbr, 1.5) - 1.4).abs() < 1e-6);
        assert!((normalize_scale_factor(ScalingType::Xbr, 1.8) - 1.8).abs() < 1e-6);

        assert!((normalize_scale_factor(ScalingType::Fsr, 1.3336) - 1.334).abs() < 1e-6);
        assert_eq!(normalize_scale_factor(ScalingType::Fsr, f32::NAN), 1.0);
    }

    #[test]
    fn matches_profile_path_handles_globs_and_titles() {
        assert!(matches_profile_path(
            "eldenring.exe;ELDEN RING*",
            r"C:\Games\ELDEN RING\Game\eldenring.exe",
            "ELDEN RING"
        ));
        assert!(matches_profile_path(
            r"C:\Games\*\bin\game?.exe",
            r"C:\Games\Test\bin\game1.exe",
            "Game"
        ));
        assert!(matches_profile_path(
            "Hollow Knight*",
            r"C:\Apps\hk.exe",
            "Hollow Knight Silksong"
        ));
        assert!(!matches_profile_path("", r"C:\Games\foo.exe", "Foo"));
        assert!(!matches_profile_path(
            "other.exe",
            r"C:\Games\foo.exe",
            "Foo"
        ));
    }

    #[test]
    fn app_settings_json_roundtrip_and_default_profile() {
        let tmp = std::env::temp_dir().join(format!(
            "flowscale_settings_test_{}.json",
            std::process::id()
        ));
        let mut settings = AppSettings {
            theme: ThemeMode::Dark,
            hotkey: HotkeyConfig {
                key: "F10".to_owned(),
                vk: 0x79,
                ctrl: true,
                alt: false,
                shift: true,
                win: false,
            },
            ..Default::default()
        };
        let custom = GameProfile {
            id: "cyber".to_owned(),
            title: "Cyber".to_owned(),
            path: "cyber.exe".to_owned(),
            scaling_type: flowscale_abi::ScalingType::Ls1,
            ls1_sharpness: 4,
            crop_input: true,
            crop_top: 12,
            ..Default::default()
        };
        settings.profiles = vec![custom.clone()];

        settings.save_to_path(&tmp).expect("save_to_path failed");
        let loaded = AppSettings::load_from_path(&tmp).expect("load_from_path failed");
        let _ = std::fs::remove_file(&tmp);

        assert_eq!(loaded.profiles[0].id, "default");
        assert_eq!(loaded.profiles[1].id, "cyber");
        assert_eq!(loaded.theme, ThemeMode::Dark);
        assert_eq!(loaded.hotkey.vk, 0x79);
        assert!(loaded
            .find_matching_profile(r"D:\Games\cyber.exe", "")
            .is_some());

        let abi = custom.to_scaling_profile();
        assert_eq!(abi.scaling_type, flowscale_abi::ScalingType::Ls1);
        assert_eq!(abi.sharpness, 4);
        assert_eq!(abi.capture_offset_top, 12);
    }

    #[test]
    fn import_lossless_settings_xml_parses_hotkey_and_profiles() {
        let xml = r#"
<Settings>
  <Hotkey>F9</Hotkey>
  <HotkeyModifiers>Control, Shift</HotkeyModifiers>
  <GameProfiles>
    <Profile>
      <Title>Default</Title>
      <ScalingType>LS1</ScalingType>
      <Ls1Type>Performance</Ls1Type>
      <LS1Sharpness>3</LS1Sharpness>
      <FrameGeneration>LSFG3</FrameGeneration>
      <LSFG3Mode>Adaptive</LSFG3Mode>
      <LSFG3Target>144</LSFG3Target>
      <LSFGFlowScale>75</LSFGFlowScale>
      <LSFGSize>Performance</LSFGSize>
      <SyncMode>Off</SyncMode>
      <CaptureApi>WGC</CaptureApi>
    </Profile>
    <Profile>
      <Title>Pixel Game</Title>
      <Path>pixel.exe;Pixel Game</Path>
      <AutoScale>true</AutoScale>
      <AutoScaleDelay>3</AutoScaleDelay>
      <WindowedMode>true</WindowedMode>
      <ScalingMode>Custom</ScalingMode>
      <ScalingType>xBR</ScalingType>
      <ScaleFactor>1.5</ScaleFactor>
      <AdjustCursorSpeed>true</AdjustCursorSpeed>
    </Profile>
  </GameProfiles>
</Settings>"#;

        let (hotkey, profiles) = import_lossless_settings_xml(xml).expect("XML import failed");
        let hk = hotkey.expect("expected hotkey");
        assert_eq!(hk.key, "F9");
        assert_eq!(hk.vk, 0x78);
        assert!(hk.ctrl && hk.shift && !hk.alt && !hk.win);

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].id, "default");
        assert_eq!(profiles[0].scaling_type, flowscale_abi::ScalingType::Ls1);
        assert_eq!(profiles[0].ls1_type, flowscale_abi::Ls1Subtype::Performance);
        assert_eq!(profiles[0].ls1_sharpness, 3);
        assert_eq!(
            profiles[0].frame_generation,
            flowscale_abi::FrameGenerationType::Lsfg3
        );
        assert_eq!(profiles[0].lsfg3_mode, flowscale_abi::Lsfg3Mode::Adaptive);
        assert_eq!(profiles[0].lsfg3_target, 144.0);
        assert_eq!(profiles[0].lsfg_flow_scale, 75);
        assert_eq!(
            profiles[0].lsfg_size,
            flowscale_abi::FrameGenerationSize::Performance
        );
        assert_eq!(profiles[0].sync_mode, flowscale_abi::SyncMode::Off);
        assert_eq!(profiles[0].capture_api, flowscale_abi::CaptureApi::Wgc);

        assert_eq!(profiles[1].id, "pixel-game");
        assert!(profiles[1].auto_scale);
        assert_eq!(profiles[1].auto_scale_delay, 3);
        assert!(profiles[1].windowed_mode);
        assert_eq!(profiles[1].scaling_type, flowscale_abi::ScalingType::Xbr);
        assert!((profiles[1].scale_factor - 1.4).abs() < 1e-6);
        assert!(profiles[1].adjust_cursor_speed);
    }
}
