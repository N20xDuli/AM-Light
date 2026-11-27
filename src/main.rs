#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    am_light_lib::logger::init();
    am_light_lib::run()
}

#[cfg(test)]
mod tests {
    use am_light_lib::am_agent::{format_audio_traits_badge, parse_track_from_am_json};
    use am_light_lib::am_api::{
        convert_to_musics_uri, extract_best_dev_token, normalize_user_token,
    };
    use am_light_lib::app_ui::{AppSettings, AppUi, FpsCap};
    use am_light_lib::audio_viz::{
        build_blackman_harris_window, compute_fft_dbfs, compute_spectrum_32,
        estimate_cutoff_from_bins, parse_enhanced_hls_variants,
        parse_enhanced_hls_variants_with_endpoint, ForensicAccumulator,
    };
    use am_light_lib::binaural_dsp::{BinauralConfig, BinauralEngine};
    use am_light_lib::bplist::{decode_bplist_hex, empty_dict_hex, encode_bplist_hex, PlistValue};
    use am_light_lib::downloads::{decode_file_url_to_path, parse_movpkg_boot_xml};
    use am_light_lib::lyrics::{
        parse_lrc_lyrics, parse_timestamp_str, parse_ttml_lyrics, parse_yrc_lyrics,
        LyricsAnimator, Spring1D,
    };
    use am_light_lib::theme::Theme;
    use am_light_lib::visualizers::VizStyle;
    use base64::{engine::general_purpose, Engine as _};
    use serde_json::json;
    use std::f32::consts::PI;

    fn fake_jwt(header_json: &str, payload_json: &str) -> String {
        let h = general_purpose::URL_SAFE_NO_PAD.encode(header_json.as_bytes());
        let p = general_purpose::URL_SAFE_NO_PAD.encode(payload_json.as_bytes());
        let s = general_purpose::URL_SAFE_NO_PAD.encode(b"signature-bytes-for-test");
        format!("{h}.{p}.{s}")
    }

    #[test]
    fn test_normalize_user_token_variants() {
        assert_eq!(normalize_user_token("  AbC123+x/y==  "), "AbC123+x/y==");
        assert_eq!(
            normalize_user_token(r#"media-user-token="AbC123%2Bx%2Fy%3D%3D";"#),
            "AbC123+x/y=="
        );
        assert_eq!(
            normalize_user_token(
                "Cookie: geo=CN; itvt=foo; media-user-token=TokenVal%2B1%3D; other=bar"
            ),
            "TokenVal+1="
        );
        assert_eq!(
            normalize_user_token("Media-User-Token: DirectHeaderValue==\n"),
            "DirectHeaderValue=="
        );
        assert_eq!(normalize_user_token("   \"\"   "), "");
    }

    #[test]
    fn test_extract_best_dev_token_handles_eyj0e_and_expiry() {
        let expired = fake_jwt(
            r#"{"typ":"JWT","alg":"ES256","kid":"WebPlayKid"}"#,
            r#"{"iss":"AMPWebPlay","exp":1000}"#,
        );
        let non_amp = fake_jwt(
            r#"{"alg":"ES256","typ":"JWT","kid":"OtherKid"}"#,
            r#"{"iss":"Other","exp":9999999999}"#,
        );
        let current_amp = fake_jwt(
            r#"{"typ":"JWT","alg":"ES256","kid":"WebPlayKid"}"#,
            r#"{"iss":"AMPWebPlay","exp":9999999999}"#,
        );
        let bundle = format!("var a=\"{expired}\"; var b=\"{non_amp}\"; var c=\"{current_amp}\";");
        let picked = extract_best_dev_token(&bundle, 5000).expect("should find valid JWT");
        assert_eq!(picked.token, current_amp);
        assert_eq!(picked.kid.as_deref(), Some("WebPlayKid"));
    }

    #[test]
    fn test_convert_to_musics_uri() {
        assert_eq!(
            convert_to_musics_uri("https://music.apple.com/cn/album/test/123").unwrap(),
            "musics://music.apple.com/cn/album/test/123"
        );
        assert_eq!(
            convert_to_musics_uri("music://music.apple.com/us/song/456").unwrap(),
            "musics://music.apple.com/us/song/456"
        );
        assert!(convert_to_musics_uri("https://evil.example.com/foo").is_err());
    }

    #[test]
    fn test_bplist_roundtrip_cjk_nested_playback_command() {
        let cmd = PlistValue::dict([
            ("cid", PlistValue::UInt(105)),
            ("subsystem-id", PlistValue::Int(2)),
            ("command-id", PlistValue::Int(5000)),
            ("negativeCheck", PlistValue::Int(-42)),
            ("flag", PlistValue::Bool(true)),
            (
                "command-params",
                PlistValue::dict([
                    ("action", PlistValue::String("play".to_string())),
                    ("startTime", PlistValue::Real(30.25)),
                    (
                        "storeParams",
                        PlistValue::dict([(
                            "items",
                            PlistValue::Array(vec![PlistValue::dict([(
                                "name",
                                PlistValue::String("Hong Kong (feat. 王菲季候風)".to_string()),
                            )])]),
                        )]),
                    ),
                ]),
            ),
        ]);

        let hex = encode_bplist_hex(&cmd);
        assert!(hex.starts_with("62706C6973743030"));
        let decoded = decode_bplist_hex(&hex).expect("should decode bplist hex");
        assert_eq!(decoded.get("command-id").and_then(PlistValue::as_i64), Some(5000));
        assert_eq!(decoded.get("negativeCheck").and_then(PlistValue::as_i64), Some(-42));
        let empty = decode_bplist_hex(&empty_dict_hex()).expect("empty dict");
        assert_eq!(empty.as_dict().map(|m| m.len()), Some(0));
    }

    #[test]
    fn test_parse_track_and_audio_traits_badge() {
        let sample = json!({
            "id": "i.lib123",
            "attributes": {
                "name": "晴天",
                "artistName": "周杰伦",
                "albumName": "叶惠美",
                "durationInMillis": 269000,
                "audioTraits": ["hi-res-lossless", "lossless", "atmos"],
                "artwork": { "url": "https://is1-ssl.mzstatic.com/image/thumb/Music/{w}x{h}bb.jpg" },
                "playParams": { "id": "i.lib123", "isLibrary": true, "catalogId": "1440818842" }
            }
        });
        let parsed = parse_track_from_am_json(&sample, None, None).expect("should parse track");
        assert_eq!(parsed.effective_adam_id(), "1440818842");
        assert_eq!(
            format_audio_traits_badge(&parsed.audio_traits).as_deref(),
            Some("Hi-Res 无损 · 杜比全景声")
        );
    }

    #[test]
    fn test_parse_enhanced_hls_variants_all_tiers() {
        let meta_json = r#"{"var_hires":{"SAMPLE-RATE":88200,"BIT-DEPTH":24,"CHANNEL-COUNT":"2"},"var_lossless":{"SAMPLE-RATE":48000,"BIT-DEPTH":24,"CHANNEL-COUNT":"2"}}"#;
        let meta_b64 = general_purpose::STANDARD.encode(meta_json.as_bytes());
        let sample_m3u8 = format!(
            "#EXTM3U\n\
             #EXT-X-VERSION:8\n\
             #EXT-X-SESSION-DATA:DATA-ID=\"com.apple.hls.audioAssetMetadata\",VALUE=\"{meta_b64}\"\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-alac-stereo-88200-24\",NAME=\"Main\",CHANNELS=\"2\",SAMPLE-RATE=88200,BIT-DEPTH=24\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-alac-stereo-48000-24\",NAME=\"Main\",CHANNELS=\"2\",SAMPLE-RATE=48000,BIT-DEPTH=24\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-atmos-2768\",NAME=\"Main\",CHANNELS=\"16/JOC\",SAMPLE-RATE=48000\n\
             #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-stereo-256\",NAME=\"Main\",CHANNELS=\"2\",SAMPLE-RATE=44100\n\
             #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=2775466,BANDWIDTH=2904120,CODECS=\"alac\",AUDIO=\"audio-alac-stereo-88200-24\",STABLE-VARIANT-ID=\"var_hires\"\n\
             hires.m3u8\n\
             #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=1776100,BANDWIDTH=1890000,CODECS=\"alac\",AUDIO=\"audio-alac-stereo-48000-24\",STABLE-VARIANT-ID=\"var_lossless\"\n\
             lossless.m3u8\n\
             #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=770200,BANDWIDTH=812000,CODECS=\"ec-3\",AUDIO=\"audio-atmos-2768\",STABLE-VARIANT-ID=\"var_atmos\"\n\
             atmos.m3u8\n\
             #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=261800,BANDWIDTH=275000,CODECS=\"mp4a.40.2\",AUDIO=\"audio-stereo-256\",STABLE-VARIANT-ID=\"var_aac\"\n\
             aac256.m3u8\n"
        );

        let hires = parse_enhanced_hls_variants(&sample_m3u8, true, 20, 10).expect("hires specs");
        assert_eq!(hires.codec, "ALAC");
        assert_eq!(hires.sample_rate_hz, 88_200);
        assert_eq!(hires.bit_depth, Some(24));

        let atmos_fb =
            parse_enhanced_hls_variants_with_endpoint(&sample_m3u8, true, 20, 20, false)
                .expect("atmos fallback specs");
        assert_eq!(atmos_fb.codec, "ALAC");
        assert!(atmos_fb.atmos_fallback_reason.is_some());
    }

    #[test]
    fn test_compute_spectrum_32_and_forensic_fft_inspector() {
        let sr = 48_000.0f32;
        let samples_512: Vec<f32> = (0..512)
            .map(|n| 0.85 * (2.0 * PI * 1000.0 * (n as f32) / sr).sin())
            .collect();
        let spec = compute_spectrum_32(&samples_512, sr);
        let (max_idx, &max_val) = spec
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap();
        assert_eq!(max_idx, 15);
        assert!(max_val > 0.75);

        // Verify merged Audio Inspector 4096-pt Blackman-Harris brickwall cutoff detection!
        let mut re = vec![0.0f32; 4096];
        let mut im = vec![0.0f32; 4096];

        // Synthesize multi-tone signal from 1 kHz up to 16.0 kHz (simulating 128 kbps MP3 brickwall at 16 kHz)
        let mut sig_16k = vec![0.0f32; 4096];
        for f_khz in 1..=16 {
            let f = (f_khz as f32) * 1000.0;
            for (n, s) in sig_16k.iter_mut().enumerate() {
                *s += 0.04 * (2.0 * PI * f * (n as f32) / sr).sin();
            }
        }
        let mut acc = ForensicAccumulator::new(48_000);
        for _ in 0..6 {
            acc.process_fft_window(&sig_16k);
        }
        let (tier, _, _, conf, _, cutoff_hz, drop_db, has_cut) = acc.evaluate_verdict();
        assert!(
            (15_800.0..=16_500.0).contains(&cutoff_hz),
            "expected ~16kHz brickwall cutoff, got {cutoff_hz}"
        );
        assert!(has_cut && drop_db > 15.0);
        assert!(conf >= 42);
        assert!(tier.starts_with("lossy"));

        // Verify full-bandwidth lossless signal up to 23.5 kHz is classified as lossless_48k!
        let mut sig_lossless = vec![0.0f32; 4096];
        for k in 1..=47 {
            let f = (k as f32) * 500.0; // 500 Hz .. 23,500 Hz
            for (n, s) in sig_lossless.iter_mut().enumerate() {
                *s += 0.02 * (2.0 * PI * f * (n as f32) / sr + (k as f32) * 0.7).sin();
            }
        }
        let mut acc_ll = ForensicAccumulator::new(48_000);
        for _ in 0..6 {
            acc_ll.process_fft_window(&sig_lossless);
        }
        let (tier_ll, _, _, _, _, cut_ll, _, _) = acc_ll.evaluate_verdict();
        assert_eq!(tier_ll, "lossless_48k", "cut_ll={cut_ll}");
        assert!(cut_ll >= 23_150.0);

        let raw_db = compute_fft_dbfs(&samples_512, &build_blackman_harris_window(512), &mut re[..512], &mut im[..512]);
        assert_eq!(raw_db.len(), 256);
        let _ = estimate_cutoff_from_bins(&raw_db, 24_000.0);
    }

    #[test]
    fn test_apple_music_ttml_yrc_lrc_and_specs_spring_physics() {
        assert_eq!(parse_timestamp_str("01:23.450"), Some(83.45));
        assert_eq!(parse_timestamp_str("12.5s"), Some(12.5));
        assert_eq!(parse_timestamp_str("8500ms"), Some(8.5));

        // 1. Apple Music TTML with syllables, inter-word spaces, x-bg backing vocals, duet v2, and translations!
        let sample_ttml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xmlns:itunes="http://music.apple.com/lyric-ttml-internal">
          <head>
            <metadata>
              <ttm:agent type="person" xml:id="v1"/>
              <ttm:agent type="person" xml:id="v2"/>
              <iTunesMetadata>
                <songwriters><songwriter>Tom Misch</songwriter><songwriter>Yussef Dayes</songwriter></songwriters>
                <translations><translation xml:lang="zh-Hans-CN"><text for="L1">红月升起</text></translation></translations>
              </iTunesMetadata>
            </metadata>
          </head>
          <body>
            <div>
              <p begin="00:08.500" end="00:12.200" itunes:key="L1" ttm:agent="v1">
                <span begin="00:08.500" end="00:09.100">Red</span> <span begin="00:09.150" end="00:10.400">moon</span> <span begin="00:10.450" end="00:12.100">rising</span>
                <span ttm:role="x-bg"><span begin="00:11.000" end="00:12.200">(rising high)</span></span>
              </p>
              <p begin="00:20.000" end="00:23.500" itunes:key="L2" ttm:agent="v2">
                <span begin="00:20.000" end="00:21.200">In</span> <span begin="00:21.200" end="00:23.500">the night</span>
                <span ttm:role="x-translation">在夜色之中</span>
              </p>
            </div>
          </body>
        </tt>"#;

        let parsed_ttml = parse_ttml_lyrics(sample_ttml, "Apple Music TTML").expect("parse ttml");
        assert!(parsed_ttml.is_syllable_synced);
        assert_eq!(parsed_ttml.songwriters, vec!["Tom Misch", "Yussef Dayes"]);
        assert_eq!(parsed_ttml.lines.len(), 2);
        assert_eq!(parsed_ttml.lines[0].text, "Red moon rising");
        assert_eq!(parsed_ttml.lines[0].translation.as_deref(), Some("红月升起"));
        assert_eq!(parsed_ttml.lines[0].bg_text.as_deref(), Some("(rising high)"));
        assert!(!parsed_ttml.lines[0].is_duet_secondary);
        assert!(parsed_ttml.lines[0].syllables[1].is_sustained); // 1.25s sustained "moon "
        assert!(parsed_ttml.lines[1].is_duet_secondary); // v2 duet right-aligned!
        assert_eq!(parsed_ttml.lines[1].translation.as_deref(), Some("在夜色之中"));

        // Verify `SyncedLyricsViewController.Specs` Animator:
        // - Intro >= 7.0s (8.5s) triggers 3-dot instrumental countdown at t=3.0s
        // - Gap between L1 (12.2s) and L2 (20.0s) = 7.8s >= 7.0s triggers interlude countdown at t=15.0s
        // - Cascade stagger (`Specs.lineDelay = 0.05s`) & syllable lift (`-2.4px`) activate smoothly!
        let mut anim = LyricsAnimator::default();
        anim.bind_lyrics("test:track", &parsed_ttml);
        anim.step(&parsed_ttml, 3.0, 700.0, 0.016);
        assert!(
            anim.interlude.visible && anim.interlude.after_line_idx.is_none(),
            "8.5s intro >= 7.0s must display 3-dot intro countdown"
        );

        anim.step(&parsed_ttml, 9.3, 700.0, 0.016);
        assert_eq!(anim.primary_active_idx, Some(0));
        for _ in 0..30 {
            anim.step(&parsed_ttml, 9.3, 700.0, 0.016);
        }
        assert!(
            anim.line_states[0].syllable_lifts[0].value < -1.0,
            "sung syllable must lift upward via liftSpring (m=1, k=14, c=7)"
        );
        for _ in 0..45 {
            anim.step(&parsed_ttml, 9.3, 700.0, 0.016);
        }
        assert!(
            (anim.line_states[0].syllable_lifts[0].value - (-2.0)).abs() < 0.15,
            "liftSpring must converge to -Specs.syllableLift (-2.0px)"
        );

        anim.step(&parsed_ttml, 15.0, 700.0, 0.016);
        assert!(
            anim.interlude.visible && anim.interlude.after_line_idx == Some(0),
            "7.8s mid-song gap >= 7.0s must display 3-dot interlude countdown"
        );

        // 2. NetEase YRC word-by-word + tlyric
        let yrc = "[12000,3200](12000,600,0)晴(12600,800,0)天(13400,1800,0)的雨\n";
        let tlyr = "[00:12.00]Rain on a sunny day\n";
        let pyrc = parse_yrc_lyrics(yrc, Some(tlyr)).expect("parse yrc");
        assert!(pyrc.is_syllable_synced);
        assert_eq!(pyrc.lines[0].text, "晴天的雨");
        assert_eq!(pyrc.lines[0].translation.as_deref(), Some("Rain on a sunny day"));
        assert_eq!(pyrc.lines[0].syllables.len(), 3);

        // 3. LRC with duplicate-timestamp bilingual merge
        let lrc = "[00:05.00]Hello world\n[00:05.00]你好世界\n[00:10.00]Second line\n";
        let plrc = parse_lrc_lyrics(lrc, None, "LRC").expect("parse lrc");
        assert_eq!(plrc.lines.len(), 2);
        assert_eq!(plrc.lines[0].translation.as_deref(), Some("你好世界"));

        // 4. Spring1D convergence check
        let mut sp = Spring1D::line_change(0.0);
        sp.target = 100.0;
        for _ in 0..120 {
            sp.step(1.0 / 60.0);
        }
        assert!((sp.value - 100.0).abs() < 0.05 && sp.is_settled());
    }

    #[test]
    fn test_apple_design_skill_wcag_contrast_and_copy_restraint() {
        // Verify all text tokens pass WCAG AA 4.5:1 against BG_PRIMARY and BG_ELEVATED (`accessibility.md`)
        assert!(
            Theme::TEXT_PRIMARY.contrast_ratio(Theme::BG_PRIMARY) >= 15.0,
            "primary text contrast too low"
        );
        assert!(
            Theme::TEXT_SECONDARY.contrast_ratio(Theme::BG_PRIMARY) >= 4.5,
            "secondary text must pass 4.5:1 WCAG AA: {}",
            Theme::TEXT_SECONDARY.contrast_ratio(Theme::BG_PRIMARY)
        );
        assert!(
            Theme::TEXT_SECONDARY.contrast_ratio(Theme::BG_ELEVATED) >= 4.5,
            "secondary text on elevated card must pass 4.5:1 WCAG AA: {}",
            Theme::TEXT_SECONDARY.contrast_ratio(Theme::BG_ELEVATED)
        );
        assert!(
            Theme::TEXT_MUTED.contrast_ratio(Theme::BG_PRIMARY) >= 4.5,
            "muted caption text must pass 4.5:1 WCAG AA: {}",
            Theme::TEXT_MUTED.contrast_ratio(Theme::BG_PRIMARY)
        );
        assert!(
            Theme::HERO_PLAY_FG.contrast_ratio(Theme::HERO_PLAY_BG) >= 18.0,
            "hero white pill button must have high contrast"
        );

        // Verify "无头" boastful copywriting is completely absent from all UI source files!
        for rel in [
            "src/app_ui.rs",
            "src/visualizers.rs",
            "src/lyrics_view.rs",
            "src/am_agent.rs",
        ] {
            let content = std::fs::read_to_string(rel).expect(rel);
            assert!(
                !content.contains("无头"),
                "found forbidden '无头' phrasing in {rel}"
            );
        }
    }

    #[test]
    fn test_parse_movpkg_boot_xml_and_binaural_dsp() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <HLSMoviePackage><Streams>
          <Stream ID="0-1145626-BBB" NetworkURL="https://aod.itunes.apple.com/P1_audio_en_gr1411_alac.m3u8">
            <Complete>YES</Complete>
          </Stream>
        </Streams></HLSMoviePackage>"#;
        let parsed = parse_movpkg_boot_xml(xml);
        assert!(parsed.complete);
        assert_eq!(parsed.format, "无损 ALAC");

        let p = decode_file_url_to_path(
            "file://localhost/C:/Users/demo/Music/Apple%20Music/Apple%20Music%20Library.musiclibrary/",
        )
        .expect("decoded path");
        assert!(p.to_string_lossy().contains("Apple Music Library.musiclibrary"));

        let cfg = BinauralConfig::from_preset("apple_spatial", true, String::new(), String::new());
        let mut eng = BinauralEngine::new(48_000, cfg);
        let mut ir = vec![[0.0f32; 2]; 512];
        ir[0] = [0.8, 0.0];
        eng.process_stereo_block(&mut ir);
        assert!(ir[0][0].abs() > 0.3);
    }

    #[test]
    fn test_edge_cases_empty_inputs_silence_fps_caps_and_hidden_spatial() {
        // 1. Empty & malformed lyrics / timestamp inputs must never panic
        assert_eq!(parse_timestamp_str(""), None);
        assert_eq!(parse_timestamp_str("   "), None);
        assert_eq!(parse_timestamp_str("invalid:ts"), None);
        assert!(parse_ttml_lyrics("", "TTML").is_none());
        assert!(parse_ttml_lyrics("<tt><body><div><p begin=\"bad\">unclosed", "TTML").is_none());
        assert!(parse_yrc_lyrics("", None).is_none());
        assert!(parse_yrc_lyrics("not a yrc line at all", Some("")).is_none());
        assert!(parse_lrc_lyrics("", None, "LRC").is_none());
        assert!(parse_lrc_lyrics("[ar:Artist Only]\n[ti:No timed lines]\n", None, "LRC").is_none());

        // 2. Empty & pure-silence DSP / FFT inputs must produce finite values without NaN
        let empty_spec = compute_spectrum_32(&[], 48_000.0);
        assert!(empty_spec.iter().all(|&v| v == 0.0));
        let mut short_re = [0.0f32; 2];
        let mut short_im = [0.0f32; 2];
        let fallback_db = compute_fft_dbfs(&[], &[1.0; 4], &mut short_re, &mut short_im);
        assert!(fallback_db.iter().all(|v| v.is_finite() && *v <= -109.0));

        let mut acc_sil = ForensicAccumulator::new(48_000);
        let silence_4096 = vec![0.0f32; 4096];
        for _ in 0..8 {
            let (inst_db, cut) = acc_sil.process_fft_window(&silence_4096);
            assert!(inst_db.iter().all(|v| v.is_finite()));
            assert_eq!(cut, 0.0);
        }
        let (tier_sil, _, _, _, _, cut_sil, _, has_bw_sil) = acc_sil.evaluate_verdict();
        assert_eq!(tier_sil, "analyzing");
        assert_eq!(cut_sil, 0.0);
        assert!(!has_bw_sil);

        // 3. 96 kHz Hi-Res 8192-point FFT accumulator (> 25.2 kHz bandwidth -> hires_lossless)
        let mut acc_96k = ForensicAccumulator::new(96_000);
        assert_eq!(acc_96k.fft_len(), 8192);
        let mut sig_96k = vec![0.0f32; 8192];
        for k in 1..=36 {
            let f = (k as f32) * 1000.0; // 1 kHz .. 36 kHz
            for (n, s) in sig_96k.iter_mut().enumerate() {
                *s += 0.022 * (2.0 * PI * f * (n as f32) / 96_000.0 + (k as f32) * 0.3).sin();
            }
        }
        for _ in 0..6 {
            acc_96k.process_fft_window(&sig_96k);
        }
        let (tier_96k, _, _, _, _, cut_96k, _, _) = acc_96k.evaluate_verdict();
        assert_eq!(tier_96k, "hires_lossless", "expected hires_lossless, cut={cut_96k}");
        assert!(cut_96k >= 35_500.0);

        // 4. Visualizer FPS cap options (>= 60 Hz up to VSync) & all 5 styles
        for cap in FpsCap::ALL {
            if let Some(ns) = cap.target_interval_ns() {
                assert!(
                    ns <= 16_666_667,
                    "FPS cap {:?} interval {ns}ns must be >= 60 Hz",
                    cap
                );
            }
        }
        assert_eq!(FpsCap::VSync.target_interval_ns(), None);
        assert_eq!(VizStyle::ALL.len(), 5);
        assert_eq!(VizStyle::AudioInspector.label(), "音质鉴真");

        // 5. Verify spatial audio flavors are hidden by default (`show_advanced_spatial == false`)
        let default_settings = AppSettings::default();
        assert_eq!(default_settings.viz_fps_cap, FpsCap::VSync);
        let ui = AppUi::new(0);
        assert!(
            !ui.show_advanced_spatial,
            "spatial audio flavor switches must be hidden/collapsed by default"
        );

        // 6. Verify audio quality detailed cards (`应有音质` / `实测音质`) exist ONLY inside `draw_audio_inspector`
        let viz_src = std::fs::read_to_string("src/visualizers.rs").expect("src/visualizers.rs");
        let split_pos = viz_src
            .find("fn draw_osu_circle")
            .expect("draw_osu_circle function");
        let non_inspector_src = &viz_src[split_pos..];
        assert!(
            !non_inspector_src.contains("应有音质") && !non_inspector_src.contains("实测音质"),
            "detailed audio quality cards must ONLY be shown in AudioInspector style"
        );
    }

    #[test]
    fn test_regressions_presets_specs_blur_aurora_sliders_and_restraint() {
        use am_light_lib::app_ui::{HitRegion, UiAction};
        use am_light_lib::lyrics::LyricsSourcePref;
        use am_light_lib::theme::RectF;

        // 1. Regression: All 4 Binaural presets (including "intimate_vocal") must roundtrip cleanly
        for preset in [
            "apple_spatial",
            "studio_nearfield",
            "concert_hall",
            "intimate_vocal",
        ] {
            let cfg = BinauralConfig::from_preset(preset, true, String::new(), String::new());
            assert_eq!(
                cfg.preset, preset,
                "preset {preset} must not fall back to default"
            );
        }

        // 2. Regression: Dolby Atmos & Spatial settings in Settings page must be inside `if self.show_advanced_spatial`
        let app_ui_src = std::fs::read_to_string("src/app_ui.rs").expect("src/app_ui.rs");
        let settings_pos = app_ui_src
            .find("fn draw_settings_page")
            .expect("draw_settings_page");
        let spatial_gate_pos = app_ui_src[settings_pos..]
            .find("if self.show_advanced_spatial")
            .expect("show_advanced_spatial gate")
            + settings_pos;
        let atmos_pos = app_ui_src[settings_pos..]
            .find("杜比全景声")
            .expect("Dolby Atmos label")
            + settings_pos;
        assert!(
            atmos_pos > spatial_gate_pos,
            "Dolby Atmos settings must be hidden inside the collapsed spatial disclosure by default"
        );

        // 3. Regression: Zero boastful "无头" / "Headless" / "原装无损引擎直出" across ALL production src/*.rs code
        for entry in std::fs::read_dir("src").expect("read src dir") {
            let path = entry.expect("dir entry").path();
            if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let full_txt = std::fs::read_to_string(&path).expect("read rs");
                let prod_txt = if path.file_name().and_then(|n| n.to_str()) == Some("main.rs") {
                    full_txt.split("#[cfg(test)]").next().unwrap_or(&full_txt)
                } else {
                    full_txt.as_str()
                };
                for forbidden in ["无头", "Headless", "headless", "原装无损引擎直出"] {
                    assert!(
                        !prod_txt.contains(forbidden),
                        "forbidden copy '{forbidden}' found in {:?}",
                        path
                    );
                }
            }
        }

        // 4. Regression: Exact `0xAD56F8` `SyncedLyricsViewController.Specs` binary constants & blur springs
        assert_eq!(LyricsAnimator::GRADIENT_FEATHER_PX, 40.0);
        assert_eq!(LyricsAnimator::SYLLABLE_LIFT_PX, 2.0);
        assert_eq!(LyricsAnimator::EMPHASIZING_SCALE_DELTA, 0.14);
        assert_eq!(LyricsAnimator::MAX_END_TIME_OFFSET_SEC, 0.50);
        assert_eq!(LyricsAnimator::BG_VOCALS_TOP_SPACING, 15.0);
        assert_eq!(LyricsAnimator::INTERLUDE_HEIGHT, 40.0);

        let multi_lrc = "[00:02.00]Line Zero\n[00:06.00]Line One\n[00:10.00]Line Two\n[00:14.00]Line Three\n";
        let parsed = parse_lrc_lyrics(multi_lrc, None, "LRC").expect("parse multi lrc");
        let mut anim = LyricsAnimator::default();
        anim.bind_lyrics("multi:blur", &parsed);
        for _ in 0..45 {
            anim.step(&parsed, 3.0, 720.0, 1.0 / 60.0);
        }
        assert_eq!(anim.primary_active_idx, Some(0));
        let b0 = anim.line_states[0].blur_spring.value;
        let b1 = anim.line_states[1].blur_spring.value;
        let b2 = anim.line_states[2].blur_spring.value;
        let b3 = anim.line_states[3].blur_spring.value;
        assert!(b0 < 0.05, "active line must be razor sharp: {b0}");
        assert!(
            b1 > 1.0 && b2 > b1 && b3 > b2,
            "inactive blur must increase monotonically with distance: {b0}, {b1}, {b2}, {b3}"
        );
        // Hovering an inactive line smoothly unblurs it
        anim.hovered_line = Some(2);
        for _ in 0..45 {
            anim.step(&parsed, 3.0, 720.0, 1.0 / 60.0);
        }
        assert!(
            anim.line_states[2].blur_spring.value < 0.1,
            "hovered inactive line must unblur smoothly"
        );

        // Source cycling roundtrip
        let mut pref = LyricsSourcePref::Auto;
        for _ in 0..5 {
            pref = pref.next();
        }
        assert_eq!(pref, LyricsSourcePref::Auto);

        // 5. Regression: SilkAurora envelope math at i=0..=144 (`u = 1.0`) must never produce NaN!
        let segs = 144usize;
        for i in 0..=segs {
            let u = (i as f32) / (segs as f32);
            let env = (u * PI).sin().max(0.0).powf(0.85);
            assert!(
                env.is_finite() && !env.is_nan(),
                "SilkAurora envelope NaN at i={i}, u={u}"
            );
        }

        // 6. Regression: Live slider click + drag scrubbing updates exact playhead without stale mouse_pos lag
        let mut ui = AppUi::new(0);
        ui.smtc.duration_sec = 200.0;
        ui.smtc.position_sec = 10.0;
        let seek_bar = RectF::new(100.0, 500.0, 400.0, 12.0);
        ui.hit_regions.push(HitRegion {
            rect: seek_bar,
            action: UiAction::ScrubMiniPlayerSeek(seek_bar),
        });
        // Mouse was previously at x=0, now directly clicks x=300 (50% of 400px bar -> 100.0s) and drags to x=400 (75% -> 150.0s)
        ui.mouse_pos = (0.0, 0.0);
        assert!(ui.on_mouse_down(300.0, 505.0));
        assert!((ui.smtc.position_sec - 100.0).abs() < 0.01);
        ui.on_mouse_move(400.0, 505.0);
        assert!((ui.smtc.position_sec - 150.0).abs() < 0.01);
        assert!(ui.on_mouse_up(400.0, 505.0));
        assert!(ui.active_drag.is_none());
    }

    #[test]
    fn test_wasapi_waveformatextensible_packed_layout_and_float_detection() {
        use am_light_lib::binaural_dsp::{
            decode_wasapi_frame_to_stereo, encode_stereo_frame_to_wasapi,
            parse_wasapi_waveformat_bytes, PackedWaveFormatEx, PackedWaveFormatExtensible,
        };

        assert_eq!(std::mem::size_of::<PackedWaveFormatEx>(), 18);
        assert_eq!(std::mem::size_of::<PackedWaveFormatExtensible>(), 40);
        assert_eq!(
            std::mem::offset_of!(PackedWaveFormatExtensible, sub_format),
            24
        );

        // Build exact 40-byte Windows WASAPI shared-mode WAVE_FORMAT_EXTENSIBLE (32-bit IEEE Float @ 48 kHz stereo)
        let mut raw_ext_float = [0u8; 40];
        raw_ext_float[0..2].copy_from_slice(&0xFFFEu16.to_le_bytes()); // wFormatTag = WAVE_FORMAT_EXTENSIBLE
        raw_ext_float[2..4].copy_from_slice(&2u16.to_le_bytes()); // nChannels = 2
        raw_ext_float[4..8].copy_from_slice(&48_000u32.to_le_bytes()); // nSamplesPerSec = 48000
        raw_ext_float[8..12].copy_from_slice(&384_000u32.to_le_bytes()); // nAvgBytesPerSec
        raw_ext_float[12..14].copy_from_slice(&8u16.to_le_bytes()); // nBlockAlign = 8
        raw_ext_float[14..16].copy_from_slice(&32u16.to_le_bytes()); // wBitsPerSample = 32
        raw_ext_float[16..18].copy_from_slice(&22u16.to_le_bytes()); // cbSize = 22
        raw_ext_float[18..20].copy_from_slice(&32u16.to_le_bytes()); // wValidBitsPerSample = 32
        raw_ext_float[20..24].copy_from_slice(&0x0000_0003u32.to_le_bytes()); // dwChannelMask = SPEAKER_FRONT_LEFT | RIGHT
        // SubFormat at exact offset 24..40 = KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {00000003-0000-0010-8000-00AA00389B71}
        raw_ext_float[24..28].copy_from_slice(&0x0000_0003u32.to_le_bytes());
        raw_ext_float[28..30].copy_from_slice(&0x0000u16.to_le_bytes());
        raw_ext_float[30..32].copy_from_slice(&0x0010u16.to_le_bytes());
        raw_ext_float[32..40].copy_from_slice(&[0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);

        let parsed = parse_wasapi_waveformat_bytes(&raw_ext_float)
            .expect("should parse 40-byte WAVE_FORMAT_EXTENSIBLE");
        assert!(
            parsed.is_float,
            "WAVE_FORMAT_EXTENSIBLE 32-bit IEEE Float must evaluate is_float == true"
        );
        assert_eq!(parsed.sample_rate, 48_000);
        assert_eq!(parsed.channels, 2);
        assert_eq!(parsed.bytes_per_sample, 4);
        assert_eq!(parsed.valid_bits_per_sample, 32);

        // Verify unaligned buffer address (+1 byte offset) also parses safely without UB
        let mut unaligned_buf = [0u8; 43];
        unaligned_buf[1..41].copy_from_slice(&raw_ext_float);
        let parsed_unaligned =
            parse_wasapi_waveformat_bytes(&unaligned_buf[1..41]).expect("unaligned parse");
        assert!(parsed_unaligned.is_float);

        // Verify WAVE_FORMAT_EXTENSIBLE 24-in-32-bit integer PCM (SubFormat = KSDATAFORMAT_SUBTYPE_PCM = 0x00000001)
        let mut raw_ext_pcm = raw_ext_float;
        raw_ext_pcm[18..20].copy_from_slice(&24u16.to_le_bytes()); // wValidBitsPerSample = 24
        raw_ext_pcm[24..28].copy_from_slice(&0x0000_0001u32.to_le_bytes()); // PCM subtype
        let parsed_pcm = parse_wasapi_waveformat_bytes(&raw_ext_pcm).expect("parse extensible pcm");
        assert!(!parsed_pcm.is_float);
        assert_eq!(parsed_pcm.valid_bits_per_sample, 24);
        assert_eq!(parsed_pcm.bytes_per_sample, 4);

        // Verify classic 18-byte WAVEFORMATEX (tag = 3 IEEE Float & tag = 1 16-bit PCM) + truncated edge cases
        let mut raw_wfx_float = [0u8; 18];
        raw_wfx_float[0..2].copy_from_slice(&3u16.to_le_bytes());
        raw_wfx_float[2..4].copy_from_slice(&2u16.to_le_bytes());
        raw_wfx_float[4..8].copy_from_slice(&44_100u32.to_le_bytes());
        raw_wfx_float[14..16].copy_from_slice(&32u16.to_le_bytes());
        assert!(parse_wasapi_waveformat_bytes(&raw_wfx_float).unwrap().is_float);

        assert!(parse_wasapi_waveformat_bytes(&[]).is_none());
        assert!(parse_wasapi_waveformat_bytes(&raw_wfx_float[..17]).is_none());
        assert!(parse_wasapi_waveformat_bytes(&raw_ext_float[..39]).is_none());

        // Zero channels or zero sample rate must safely return None
        let mut raw_zero_ch = raw_wfx_float;
        raw_zero_ch[2..4].copy_from_slice(&0u16.to_le_bytes());
        assert!(parse_wasapi_waveformat_bytes(&raw_zero_ch).is_none());
        let mut raw_zero_sr = raw_wfx_float;
        raw_zero_sr[4..8].copy_from_slice(&0u32.to_le_bytes());
        assert!(parse_wasapi_waveformat_bytes(&raw_zero_sr).is_none());

        // Verify unaligned encode/decode roundtrip across float32, 16-bit, 24-bit packed (3B), and 24-in-32 PCM
        let mut f32_frame = [0u8; 9];
        encode_stereo_frame_to_wasapi([0.42, -0.73], &parsed, &mut f32_frame[1..9]);
        let dec_f32 = decode_wasapi_frame_to_stereo(&f32_frame[1..9], &parsed);
        assert!((dec_f32[0] - 0.42).abs() < 1e-6 && (dec_f32[1] + 0.73).abs() < 1e-6);

        let mut pcm24in32_frame = [0u8; 8];
        encode_stereo_frame_to_wasapi([0.5, -0.25], &parsed_pcm, &mut pcm24in32_frame);
        // Bottom 8 padding bits must be zero-masked for 24-in-32 MSB-aligned WASAPI containers
        assert_eq!(pcm24in32_frame[0], 0);
        assert_eq!(pcm24in32_frame[4], 0);
        let dec_24in32 = decode_wasapi_frame_to_stereo(&pcm24in32_frame, &parsed_pcm);
        assert!((dec_24in32[0] - 0.5).abs() < 1e-4 && (dec_24in32[1] + 0.25).abs() < 1e-4);

        let mut raw_ext_24pack = raw_ext_pcm;
        raw_ext_24pack[12..14].copy_from_slice(&6u16.to_le_bytes()); // nBlockAlign = 6 (2ch * 3B)
        raw_ext_24pack[14..16].copy_from_slice(&24u16.to_le_bytes()); // wBitsPerSample = 24
        let parsed_24pack = parse_wasapi_waveformat_bytes(&raw_ext_24pack).expect("24-bit packed");
        assert_eq!(parsed_24pack.bytes_per_sample, 3);
        assert_eq!(parsed_24pack.block_align, 6);
        let mut i24_frame = [0u8; 6];
        encode_stereo_frame_to_wasapi([0.375, -0.625], &parsed_24pack, &mut i24_frame);
        let dec_i24 = decode_wasapi_frame_to_stereo(&i24_frame, &parsed_24pack);
        assert!((dec_i24[0] - 0.375).abs() < 1e-5 && (dec_i24[1] + 0.625).abs() < 1e-5);

        // Verify 7.1 (8-channel) WAVE_FORMAT_EXTENSIBLE ITU-R BS.775 downmix when Front L/R are quiet
        // and lead vocal / surround sits in FC (ch 2), BL (ch 4), BR (ch 5), SL (ch 6), SR (ch 7)
        let mut raw_ext_71 = raw_ext_float;
        raw_ext_71[2..4].copy_from_slice(&8u16.to_le_bytes()); // 8 channels
        raw_ext_71[12..14].copy_from_slice(&32u16.to_le_bytes()); // nBlockAlign = 32
        raw_ext_71[20..24].copy_from_slice(&0x0000_063Fu32.to_le_bytes()); // KSAUDIO_SPEAKER_7POINT1_SURROUND
        let parsed_71 = parse_wasapi_waveformat_bytes(&raw_ext_71).expect("7.1 float");
        let ch_71: [f32; 8] = [0.0, 0.0, 0.40, 0.0, 0.20, -0.10, 0.10, 0.30];
        let mut frame_71 = [0u8; 32];
        for (idx, &v) in ch_71.iter().enumerate() {
            frame_71[idx * 4..(idx + 1) * 4].copy_from_slice(&v.to_le_bytes());
        }
        let downmixed = decode_wasapi_frame_to_stereo(&frame_71, &parsed_71);
        let exp_l = 0.707_106_77 * (0.40 + 0.20 + 0.10);
        let exp_r = 0.707_106_77 * (0.40 - 0.10 + 0.30);
        assert!(
            (downmixed[0] - exp_l).abs() < 1e-5 && (downmixed[1] - exp_r).abs() < 1e-5,
            "7.1 surround/center channels must downmix cleanly into stereo: got {:?}",
            downmixed
        );
    }

    #[test]
    fn test_com_security_init_order_and_playback_delegate_events() {
        use am_light_lib::am_agent::{
            com_security_init_hr, dispatch_playback_event_plist, get_playback_state,
            init_process_com_security, playback_event_count, query_playback_delegate_iid,
        };

        // Dedicated MTA PB_DELEGATE supports IUnknown and IPlaybackEventDelegate, while
        // rejecting IID_IAgileObject and unknown IIDs with E_NOINTERFACE (-2147467262) so
        // AMPLibraryAgent wraps our proxy via RoGetAgileReference for STA -> MTA callbacks.
        assert_eq!(
            query_playback_delegate_iid(0, 0, 0, [0xC0, 0, 0, 0, 0, 0, 0, 0x46]),
            0
        );
        assert_eq!(
            query_playback_delegate_iid(
                0xF0E2_09BE,
                0x7FB1,
                0x5E1E,
                [0x98, 0x79, 0xB6, 0x22, 0xAE, 0x7C, 0xCA, 0x41]
            ),
            0
        );
        assert_eq!(
            query_playback_delegate_iid(
                0x94EA_2B94,
                0xE9CC,
                0x49E0,
                [0xC0, 0xFF, 0xEE, 0x64, 0xCA, 0x8F, 0x5B, 0x90]
            ),
            -2147467262 // E_NOINTERFACE
        );
        assert_eq!(
            query_playback_delegate_iid(
                0xDEAD_BEEF,
                0x1234,
                0x5678,
                [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]
            ),
            -2147467262 // E_NOINTERFACE
        );

        // Even when audio_viz::list_output_devices or smtc::get_smtc_state is called in any order,
        // CoInitializeSecurity MUST have succeeded with S_OK (0) rather than RPC_E_TOO_LATE (0x80010119)!
        let _ = am_light_lib::audio_viz::list_output_devices();
        let hr = init_process_com_security();
        const RPC_E_TOO_LATE: i32 = 0x8001_0119u32 as i32;
        assert_ne!(
            hr, RPC_E_TOO_LATE,
            "CoInitializeSecurity must never fail with RPC_E_TOO_LATE (0x80010119)"
        );
        assert_eq!(
            hr, 0,
            "CoInitializeSecurity expected S_OK (0), got 0x{:08X}",
            hr as u32
        );
        assert_eq!(com_security_init_hr(), 0);

        // Verify startup order in src/lib.rs: init_process_com_security() runs before restore_stale_routing_on_startup,
        // D2DRenderer::new, and AppUi::new.
        let lib_src = std::fs::read_to_string("src/lib.rs").expect("read src/lib.rs");
        let fn_start = lib_src
            .find("pub fn run_native_app()")
            .expect("run_native_app");
        let run_body = &lib_src[fn_start..];
        let sec_pos = run_body
            .find("init_process_com_security()")
            .expect("init_process_com_security call");
        let restore_pos = run_body
            .find("restore_stale_routing_on_startup()")
            .expect("restore_stale_routing_on_startup call");
        let d2d_pos = run_body
            .find("D2DRenderer::new")
            .expect("D2DRenderer::new call");
        let ui_pos = run_body.find("AppUi::new").expect("AppUi::new call");
        assert!(
            sec_pos < restore_pos && sec_pos < d2d_pos && sec_pos < ui_pos,
            "init_process_com_security must execute before any COM consumer in run_native_app"
        );

        // Verify AMPLibraryAgent playback event plist updates timeline, status, duration, and lossless badge
        let before_cnt = playback_event_count();
        let evt_play = PlistValue::dict([
            ("canBePositioned", PlistValue::Bool(true)),
            ("currentTime", PlistValue::Real(36.50)),
            ("duration", PlistValue::Real(244.0)),
            ("isPlaying", PlistValue::Bool(true)),
            ("audioFormatSampleRate", PlistValue::UInt(44_100)),
            ("audioFormatBitDepth", PlistValue::UInt(16)),
        ]);
        dispatch_playback_event_plist(&evt_play);
        assert_eq!(playback_event_count(), before_cnt + 1);

        let st1 = get_playback_state();
        assert_eq!(st1.status, "Playing");
        assert!((st1.duration_sec - 244.0).abs() < 0.01);
        assert!((36.50..=36.80).contains(&st1.position_sec));
        assert_eq!(st1.audio_quality.as_deref(), Some("ALAC 16-bit / 44.1kHz"));

        std::thread::sleep(std::time::Duration::from_millis(65));
        let st2 = get_playback_state();
        assert!(
            st2.position_sec > st1.position_sec + 0.03,
            "timeline must advance monotonically while Playing: {} -> {}",
            st1.position_sec,
            st2.position_sec
        );

        // Verify 24-bit / 96kHz Hi-Res ALAC event & Paused state transition
        let evt_hires = PlistValue::dict([
            ("canBePositioned", PlistValue::Bool(true)),
            ("currentTime", PlistValue::Real(88.25)),
            ("duration", PlistValue::Real(266.6)),
            ("isPlaying", PlistValue::Bool(true)),
            ("audioFormatSampleRate", PlistValue::UInt(96_000)),
            ("audioFormatBitDepth", PlistValue::UInt(24)),
        ]);
        dispatch_playback_event_plist(&evt_hires);
        let st_hires = get_playback_state();
        assert_eq!(st_hires.status, "Playing");
        assert!((st_hires.duration_sec - 266.6).abs() < 0.01);
        assert!((88.25..=88.55).contains(&st_hires.position_sec));
        assert_eq!(
            st_hires.audio_quality.as_deref(),
            Some("ALAC 24-bit / 96kHz")
        );

        let evt_pause = PlistValue::dict([("isPlaying", PlistValue::Bool(false))]);
        dispatch_playback_event_plist(&evt_pause);
        assert_eq!(get_playback_state().status, "Paused");

        // Reset clean Stopped state so other tests see idle state
        dispatch_playback_event_plist(&PlistValue::dict([
            ("currentTime", PlistValue::Real(0.0)),
            ("isPlaying", PlistValue::Bool(false)),
        ]));
    }

    #[test]
    fn test_forensic_acoustic_piano_intro_no_false_lossy_and_header_fit() {
        use am_light_lib::visualizers::{
            format_inspector_detected_header, inspector_detected_header_avail_w,
            INSPECTOR_HEADER_FONT_SIZE,
        };
        use windows::core::HSTRING;
        use windows::Win32::Graphics::DirectWrite::{
            DWriteCreateFactory, IDWriteFactory, DWRITE_FACTORY_TYPE_SHARED,
            DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
            DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
        };

        // 1. Simulate genuine 16-bit / 44.1 kHz ALAC piano/vocal intro (`下一个天亮` style):
        // Harmonics up to ~7.55 kHz with natural gentle rolloff (~9 dB drop, no codec brickwall)
        // quantized to exact 16-bit integer PCM grid (`k / 32768.0`).
        let sr = 44_100.0f32;
        let mut piano_intro = vec![0.0f32; 4096];
        let freqs_amps: &[(f32, f32)] = &[
            (440.0, 0.14),
            (880.0, 0.095),
            (1320.0, 0.065),
            (1760.0, 0.045),
            (2640.0, 0.026),
            (3520.0, 0.015),
            (4400.0, 0.0085),
            (5280.0, 0.0048),
            (6160.0, 0.0026),
            (6820.0, 0.0014),
            (7550.0, 0.00072),
            (8250.0, 0.00026),
            (9020.0, 0.00010),
        ];
        for (n, s) in piano_intro.iter_mut().enumerate() {
            let mut v = 0.0f32;
            for (k, &(f, a)) in freqs_amps.iter().enumerate() {
                v += a * (2.0 * PI * f * (n as f32) / sr + (k as f32) * 0.41).sin();
            }
            // Exact 16-bit lossless ALAC integer PCM quantization
            *s = (v * 32_768.0).round().clamp(-32_767.0, 32_767.0) / 32_768.0;
        }
        let stereo_pcm_16: Vec<[f32; 2]> = piano_intro.iter().map(|&x| [x, x * 1.0]).collect();

        let mut acc_piano = ForensicAccumulator::new(44_100);
        acc_piano.observe_pcm_samples(&stereo_pcm_16);
        assert_eq!(acc_piano.detected_bit_grid_label(), "16-bit PCM");
        for _ in 0..8 {
            acc_piano.process_fft_window(&piano_intro);
        }
        let (tier_p, title_p, _, _, _, cut_p, drop_p, has_bw_p) = acc_piano.evaluate_verdict();
        assert!(
            !tier_p.starts_with("lossy") && !title_p.contains("有损"),
            "16-bit ALAC piano intro must NEVER be flagged as lossy: tier={tier_p}, title={title_p}, cut={cut_p}, drop={drop_p}"
        );
        assert_eq!(tier_p, "lossless_acoustic");
        assert!(!has_bw_p);

        // 2. Same piano intro attenuated by digital volume slider (e.g. 0.73x float gain -> non-integer grid)
        let piano_float: Vec<f32> = piano_intro.iter().map(|&x| x * 0.73).collect();
        let stereo_float: Vec<[f32; 2]> = piano_float.iter().map(|&x| [x, x]).collect();
        let mut acc_float_intro = ForensicAccumulator::new(44_100);
        acc_float_intro.observe_pcm_samples(&stereo_float);
        for _ in 0..8 {
            acc_float_intro.process_fft_window(&piano_float);
        }
        let (tier_fl, title_fl, _, _, _, _, _, _) = acc_float_intro.evaluate_verdict();
        assert!(
            !tier_fl.starts_with("lossy") && !title_fl.contains("低码率有损"),
            "attenuated narrowband acoustic intro must not be flagged lossy: tier={tier_fl}, title={title_fl}"
        );
        assert_eq!(tier_fl, "narrowband_analyzing");

        // 3. Once broadband percussion/cymbals enter on the 44.1 kHz lossless stream, it locks lossless_cd!
        let mut full_cd = vec![0.0f32; 4096];
        for k in 1..=43 {
            let f = (k as f32) * 500.0; // 500 Hz .. 21,500 Hz
            for (n, s) in full_cd.iter_mut().enumerate() {
                *s += 0.018 * (2.0 * PI * f * (n as f32) / sr + (k as f32) * 0.5).sin();
            }
        }
        for _ in 0..6 {
            acc_piano.process_fft_window(&full_cd);
        }
        let (tier_cd, _, _, _, _, cut_cd, _, _) = acc_piano.evaluate_verdict();
        assert_eq!(tier_cd, "lossless_cd", "expected lossless_cd after percussion, cut={cut_cd}");

        // 4. Fake-lossless transcode (128k MP3 hard brickwall at 15.8 kHz re-saved as 16-bit PCM) MUST still be caught as lossy_low!
        let mut fake_ll = vec![0.0f32; 4096];
        for k in 1..=31 {
            let f = (k as f32) * 500.0; // 500 Hz .. 15,500 Hz hard stop
            for (n, s) in fake_ll.iter_mut().enumerate() {
                *s += 0.025 * (2.0 * PI * f * (n as f32) / sr + (k as f32) * 0.3).sin();
            }
        }
        for s in &mut fake_ll {
            *s = (*s * 32_768.0).round() / 32_768.0;
        }
        let fake_stereo: Vec<[f32; 2]> = fake_ll.iter().map(|&x| [x, x]).collect();
        let mut acc_fake = ForensicAccumulator::new(44_100);
        acc_fake.observe_pcm_samples(&fake_stereo);
        for _ in 0..6 {
            acc_fake.process_fft_window(&fake_ll);
        }
        let (tier_fk, _, _, _, _, _, _, has_bw_fk) = acc_fake.evaluate_verdict();
        assert!(has_bw_fk && tier_fk == "lossy_low", "fake 128k transcode must still be caught: {tier_fk}");

        // 4b. Real-world pink-sloped (-6 dB/oct) 128k MP3 transcode (where 500 Hz is ~30 dB louder than 15.5 kHz)
        // MUST also be caught as a genuine codec brickwall!
        let mut pink_mp3 = vec![0.0f32; 4096];
        for k in 1..=31 {
            let f = (k as f32) * 500.0; // 500 Hz .. 15,500 Hz with -6 dB/oct natural musical spectral tilt
            let amp = 0.16 * (500.0 / f);
            for (n, s) in pink_mp3.iter_mut().enumerate() {
                *s += amp * (2.0 * PI * f * (n as f32) / sr + (k as f32) * 0.37).sin();
            }
        }
        for s in &mut pink_mp3 {
            *s = (*s * 32_768.0).round() / 32_768.0;
        }
        let pink_stereo: Vec<[f32; 2]> = pink_mp3.iter().map(|&x| [x, x]).collect();
        let mut acc_pink = ForensicAccumulator::new(44_100);
        acc_pink.observe_pcm_samples(&pink_stereo);
        for _ in 0..6 {
            acc_pink.process_fft_window(&pink_mp3);
        }
        let (tier_pk, _, _, _, _, cut_pk, drop_pk, has_bw_pk) = acc_pink.evaluate_verdict();
        assert!(
            has_bw_pk && tier_pk == "lossy_low",
            "pink-sloped 128k MP3 brickwall must be caught: tier={tier_pk}, cut={cut_pk}, drop={drop_pk}"
        );

        // 4c. Volume-attenuated (0.14x = -17.1 dB) pink-sloped 128k MP3 (including 16-bit PCM fake-lossless!)
        // MUST STILL be caught as lossy_low rather than slipping below hardcoded passband dBFS thresholds!
        let quiet_pink: Vec<f32> = pink_mp3.iter().map(|&x| (x * 0.14 * 32_768.0).round() / 32_768.0).collect();
        let quiet_pink_stereo: Vec<[f32; 2]> = quiet_pink.iter().map(|&x| [x, x]).collect();
        let mut acc_quiet_pink = ForensicAccumulator::new(44_100);
        acc_quiet_pink.observe_pcm_samples(&quiet_pink_stereo);
        for _ in 0..6 {
            acc_quiet_pink.process_fft_window(&quiet_pink);
        }
        let (tier_qp, _, _, _, _, cut_qp, drop_qp, has_bw_qp) = acc_quiet_pink.evaluate_verdict();
        assert!(
            has_bw_qp && tier_qp == "lossy_low",
            "volume-attenuated (-17 dB) pink-sloped 128k MP3 must still be caught: tier={tier_qp}, cut={cut_qp}, drop={drop_qp}"
        );

        // 4d. Loud solo piano (0 dBFS class) with faint -74 dBFS 14 kHz harmonic must NOT trigger brickwall
        let mut loud_piano = vec![0.0f32; 4096];
        let loud_harmonics: &[(f32, f32)] = &[
            (523.25, 0.35),
            (1046.5, 0.22),
            (2093.0, 0.11),
            (4186.0, 0.045),
            (6279.0, 0.018),
            (8372.0, 0.008),
            (10465.0, 0.003),
            (12558.0, 0.0009),
            (14080.0, 0.00020), // ~ -74 dBFS faint overtone
        ];
        for (n, s) in loud_piano.iter_mut().enumerate() {
            let mut v = 0.0f32;
            for (k, &(f, a)) in loud_harmonics.iter().enumerate() {
                v += a * (2.0 * PI * f * (n as f32) / sr + (k as f32) * 0.29).sin();
            }
            *s = (v * 32_768.0).round().clamp(-32_767.0, 32_767.0) / 32_768.0;
        }
        let loud_piano_st: Vec<[f32; 2]> = loud_piano.iter().map(|&x| [x, x]).collect();
        let mut acc_loud_piano = ForensicAccumulator::new(44_100);
        acc_loud_piano.observe_pcm_samples(&loud_piano_st);
        for _ in 0..6 {
            acc_loud_piano.process_fft_window(&loud_piano);
        }
        let (tier_lp, _, _, _, _, cut_lp, drop_lp, has_bw_lp) = acc_loud_piano.evaluate_verdict();
        assert!(
            !has_bw_lp && !tier_lp.starts_with("lossy"),
            "loud solo piano with faint 14kHz overtone must not false-flag lossy: tier={tier_lp}, cut={cut_lp}, drop={drop_lp}"
        );

        // 5. Measure actual DirectWrite glyph width across EVERY window width 780..=1400 DIP (step 5 DIP,
        // plus exact 1185 DIP) in BOTH primary ("Segoe UI Variable Display") and fallback ("Microsoft YaHei UI")
        // fonts at 3-digit confidence (100%), AND confirm 1185 DIP displays the full "实时鉴定" string!
        unsafe {
            let dwrite: IDWriteFactory =
                DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("DWriteCreateFactory");

            let avail_1185 = {
                let body_w = (1185.0f32 - Theme::SIDEBAR_WIDTH) - 44.0;
                let det_card_w = (body_w - 12.0) * 0.5;
                inspector_detected_header_avail_w(det_card_w)
            };
            assert_eq!(
                format_inspector_detected_header(true, 100, avail_1185),
                "实测 · 置信度 100%"
            );

            for family_name in ["Segoe UI Variable Display", "Microsoft YaHei UI"] {
                let fmt = dwrite
                    .CreateTextFormat(
                        &HSTRING::from(family_name),
                        None,
                        DWRITE_FONT_WEIGHT(600),
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        INSPECTOR_HEADER_FONT_SIZE,
                        &HSTRING::from("zh-CN"),
                    )
                    .expect("CreateTextFormat");
                fmt.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP).unwrap();

                let mut widths: Vec<f32> = (780..=1400).step_by(5).map(|w| w as f32).collect();
                widths.push(1185.0);
                for win_w in widths {
                    let sb_w = if win_w >= Theme::SIDEBAR_AUTO_COLLAPSE_WIDTH {
                        Theme::SIDEBAR_WIDTH
                    } else {
                        0.0
                    };
                    let body_w = (win_w - sb_w).max(200.0) - 44.0;
                    let det_card_w = (body_w - 12.0) * 0.5;
                    let hdr_avail_w = inspector_detected_header_avail_w(det_card_w);
                    let hdr_text = format_inspector_detected_header(true, 100, hdr_avail_w);
                    let utf16: Vec<u16> = hdr_text.encode_utf16().collect();
                    let layout = dwrite
                        .CreateTextLayout(&utf16, &fmt, 2000.0, 24.0)
                        .expect("CreateTextLayout");
                    let mut metrics = DWRITE_TEXT_METRICS::default();
                    layout.GetMetrics(&mut metrics).expect("GetMetrics");
                    assert!(
                        metrics.widthIncludingTrailingWhitespace <= hdr_avail_w - 1.5,
                        "font '{}' header '{}' width {:.1}px exceeds available {:.1}px at window width {:.0} DIP!",
                        family_name,
                        hdr_text,
                        metrics.widthIncludingTrailingWhitespace,
                        hdr_avail_w,
                        win_w
                    );
                }
            }
        }
    }

    #[test]
    fn test_chernobyl_reactor_rods_moderate_vocal_dynamics_and_silence() {
        use am_light_lib::audio_viz::LiveVisualizerSnapshot;
        use am_light_lib::visualizers::VisualizerState;

        let mut viz = VisualizerState::default();
        viz.style = VizStyle::ChernobylRods;

        // 1. Silence: rods must stay calmly at 0.0 px
        let silent_snap = LiveVisualizerSnapshot::default();
        for _ in 0..30 {
            viz.step(&silent_snap, 1.0 / 60.0, false);
        }
        let (sil_mean, sil_max, _) = viz.rod_lift_stats();
        assert_eq!(sil_mean, 0.0);
        assert_eq!(sil_max, 0.0);
        assert!(!viz.has_settling_motion());

        // 2. Moderate -21 dBFS vocal/pop playback WITHOUT heavy sub-bass kicks (`beat_triggered = false`)
        let mut vocal_snap = LiveVisualizerSnapshot {
            active_signal: true,
            rms_db: -21.0,
            peak_db_l: -15.5,
            peak_db_r: -15.8,
            bass_energy: 0.03,
            mid_energy: 0.21,
            high_energy: 0.08,
            beat_pulse: 0.0,
            beat_triggered: false,
            ..LiveVisualizerSnapshot::default()
        };
        // Sub-bass nearly empty; vocals & piano fundamentals/harmonics in bands 14..42
        for b in 0..12 {
            vocal_snap.spectrum_64[b] = 0.02;
        }
        for b in 14..42 {
            vocal_snap.spectrum_64[b] = 0.16 + ((b % 5) as f32) * 0.024;
        }

        let mut peak_max_lift = 0.0f32;
        let mut peak_mean_lift = 0.0f32;
        let mut max_std_dev = 0.0f32;
        for f in 0..60 {
            // Slight natural syllable modulation every 12 frames
            let syl = 0.85 + 0.25 * ((f as f32) * 0.35).sin();
            vocal_snap.mid_energy = 0.20 * syl;
            viz.step(&vocal_snap, 1.0 / 60.0, false);
            let (mean_z, max_z, std_z) = viz.rod_lift_stats();
            peak_max_lift = peak_max_lift.max(max_z);
            peak_mean_lift = peak_mean_lift.max(mean_z);
            max_std_dev = max_std_dev.max(std_z);
        }
        assert!(
            peak_max_lift >= 15.0 && peak_mean_lift >= 7.0,
            "reactor rods must exhibit clear vertical lift at -21 dBFS vocal playback: max={peak_max_lift:.1}px, mean={peak_mean_lift:.1}px"
        );
        assert!(
            max_std_dev >= 2.2,
            "reactor rods must clatter differentially rather than moving as a flat sheet: std={max_std_dev:.2}px"
        );

        // 3. Soft -34 dBFS acoustic passage must also visibly oscillate (> 7 px max lift)
        let mut soft_viz = VisualizerState::default();
        let mut soft_snap = vocal_snap.clone();
        soft_snap.rms_db = -34.0;
        soft_snap.peak_db_l = -27.0;
        soft_snap.peak_db_r = -27.0;
        soft_snap.mid_energy = 0.09;
        for v in &mut soft_snap.spectrum_64 {
            *v *= 0.45;
        }
        let mut soft_max = 0.0f32;
        for _ in 0..45 {
            soft_viz.step(&soft_snap, 1.0 / 60.0, false);
            soft_max = soft_max.max(soft_viz.rod_lift_stats().1);
        }
        assert!(
            soft_max >= 8.0,
            "soft -34 dBFS passage must still visibly lift rods: soft_max={soft_max:.1}px"
        );

        // 4. Frame-rate invariance check: 240 Hz vs 60 Hz on vocal crescendo must stay consistent without runaway velocity stacking
        let mut viz_60 = VisualizerState::default();
        let mut viz_240 = VisualizerState::default();
        let mut max_60 = 0.0f32;
        let mut max_240 = 0.0f32;
        for step_ms in 0..60 {
            let env = ((step_ms as f32) / 12.0).clamp(0.0, 1.0);
            let mut cresc = vocal_snap.clone();
            cresc.mid_energy = 0.05 + 0.34 * env;
            viz_60.step(&cresc, 1.0 / 60.0, false);
            max_60 = max_60.max(viz_60.rod_lift_stats().1);
            for _ in 0..4 {
                viz_240.step(&cresc, 1.0 / 240.0, false);
                max_240 = max_240.max(viz_240.rod_lift_stats().1);
            }
        }
        assert!(
            max_240 >= 14.0 && (max_240 - max_60).abs() <= 12.0,
            "240 Hz rod lift ({max_240:.1}px) must stay consistent with 60 Hz ({max_60:.1}px)"
        );

        // 5. Transition back to silence must settle all rods cleanly to 0.0 px
        for _ in 0..45 {
            viz.step(&silent_snap, 1.0 / 60.0, false);
        }
        let (_, settled_max, _) = viz.rod_lift_stats();
        assert!(
            settled_max == 0.0,
            "rods must return to 0.0 px during silence"
        );
        assert!(!viz.has_settling_motion());
    }

    #[test]
    fn test_frameless_corner_radius_reactor_matrix_soundstage_and_loading() {
        use am_light_lib::app_ui::{HitRegion, UiAction};
        use am_light_lib::audio_viz::LiveVisualizerSnapshot;
        use am_light_lib::theme::RectF;
        use am_light_lib::visualizers::{VisualizerState, VizHitAction, VizHitZone};

        // 1. Default window corner radius = 12.0 px, supports 0.0 px right-angle & 0..28 px slider drag
        let mut ui = AppUi::new(0);
        assert!((ui.settings.window_corner_radius - 12.0).abs() < 0.01);
        ui.execute_action(UiAction::SetWindowCornerRadius(0.0));
        assert_eq!(ui.settings.window_corner_radius, 0.0);

        let rad_bar = RectF::new(200.0, 300.0, 280.0, 6.0);
        ui.hit_regions.push(HitRegion {
            rect: RectF::new(194.0, 290.0, 292.0, 28.0),
            action: UiAction::ScrubWindowCornerRadius(rad_bar),
        });
        assert!(ui.on_mouse_down(340.0, 302.0)); // 50% of 280px -> 14.0 px
        assert!((ui.settings.window_corner_radius - 14.0).abs() < 0.1);
        ui.on_mouse_move(600.0, 302.0); // clamp max -> 28.0 px
        assert!((ui.settings.window_corner_radius - 28.0).abs() < 0.01);
        assert!(ui.on_mouse_up(600.0, 302.0));

        // 2. Default Reactor style = 0 (3D Luminescent Matrix Core) + click-to-toggle -> 1 (Chernobyl RBMK)
        let mut viz = VisualizerState::default();
        assert_eq!(
            viz.reactor_sub_style, 0,
            "Reactor Core must default to 3D Luminescent Cuboid Matrix Core (style 0)"
        );
        ui.hit_regions.clear();
        ui.viz_state.style = VizStyle::ChernobylRods;
        assert_eq!(ui.viz_state.reactor_sub_style, 0);
        ui.viz_state.hit_zones.push(VizHitZone {
            rect: RectF::new(20.0, 100.0, 800.0, 500.0),
            action: VizHitAction::ToggleReactorSubStyle,
        });
        ui.route = am_light_lib::app_ui::PageRoute::Visualizer;
        let _ = ui.on_mouse_down(400.0, 300.0);
        assert!(ui.on_mouse_up(400.0, 300.0));
        assert_eq!(
            ui.viz_state.reactor_sub_style, 1,
            "Clicking reactor viewport must toggle to RBMK-1000 rods (style 1)"
        );
        let _ = ui.on_mouse_down(400.0, 300.0);
        assert!(ui.on_mouse_up(400.0, 300.0));
        assert_eq!(
            ui.viz_state.reactor_sub_style, 0,
            "Clicking reactor viewport again must toggle back to Matrix Core (style 0)"
        );

        // 3. Soundstage scatter history + balanced Left/Right stereo bars (never right-biased on centered audio!)
        let mut centered_snap = LiveVisualizerSnapshot {
            active_signal: true,
            rms_db: -18.0,
            peak_db_l: -14.0,
            peak_db_r: -14.0,
            bass_energy: 0.28,
            mid_energy: 0.32,
            high_energy: 0.14,
            phase_correlation: 0.96, // high positive phase correlation that previously skewed old single bar right!
            stereo_width_pct: 25.0,
            ..LiveVisualizerSnapshot::default()
        };
        centered_snap.soundstage_lr = (0..128)
            .map(|i| {
                let v = ((i as f32) * 0.21).sin() * 0.35;
                [v, v] // identical L & R channels
            })
            .collect();
        for b in 0..48 {
            centered_snap.spectrum_64[b] = 0.24;
        }
        for _ in 0..24 {
            viz.step(&centered_snap, 1.0 / 60.0, false);
        }
        let (hist_len, lvl_l, lvl_r) = viz.soundstage_stats();
        assert!(hist_len >= 128, "Soundstage scatter plot must accumulate points: {hist_len}");
        assert!(
            lvl_l > 0.25 && (lvl_l - lvl_r).abs() < 0.005,
            "Centered audio must produce equal Left ({lvl_l:.3}) and Right ({lvl_r:.3}) bars instead of right bias!"
        );
        let (_, mat_max) = viz.matrix_pillar_stats();
        assert!(mat_max > 8.0, "3D Matrix Core pillars must elevate dynamically on audio: {mat_max:.1}px");
    }

    #[test]
    #[ignore]
    fn live_verify_amp_agent_callbacks_forensic_and_rods() {
        use am_light_lib::am_agent::{
            com_security_init_hr, control, disconnect_agent, get_playback_state,
            init_process_com_security, play_tracks, playback_event_count,
        };
        use am_light_lib::audio_viz::{sample_live_visualizer, set_viz_subscribed};
        use am_light_lib::visualizers::VisualizerState;
        use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, IWICImagingFactory};
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
        };

        // 1. Reproduce exact run_native_app startup sequence: STA -> init_process_com_security -> WIC -> MMDevice
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let sec_hr = init_process_com_security();
            println!("[LIVE] CoInitializeSecurity HRESULT = 0x{:08X}", sec_hr as u32);
            assert_eq!(sec_hr, 0, "CoInitializeSecurity must be S_OK");
            let _wic: IWICImagingFactory =
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .expect("WIC factory");
        }
        let devs = am_light_lib::audio_viz::list_output_devices().expect("list devices");
        println!("[LIVE] Output devices ({}): {:?}", devs.len(), devs.first().map(|d| (&d.name, d.sample_rate, &d.format_tag)));
        assert_eq!(com_security_init_hr(), 0);

        // 2. Search catalog for `下一个天亮` and start live AMPLibraryAgent lossless playback
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let search_json = rt
            .block_on(am_light_lib::am_api::fetch_am_api(
                "v1/catalog/cn/search?term=%E4%B8%8B%E4%B8%80%E4%B8%AA%E5%A4%A9%E4%BA%AE&types=songs&limit=5&extend=audioTraits&l=zh-Hans-CN".to_string(),
                None,
                None,
                Some("cn".to_string()),
            ))
            .expect("catalog search");
        let items = search_json
            .pointer("/results/songs/data")
            .and_then(|v| v.as_array())
            .expect("songs array");
        let track = items
            .iter()
            .find_map(|it| parse_track_from_am_json(it, None, None))
            .expect("parsed track");
        println!(
            "[LIVE] Found track: {} - {} (id={}, traits={:?})",
            track.name, track.artist_name, track.id, track.audio_traits
        );

        let evt_before = playback_event_count();
        let conn_st = am_light_lib::am_agent::ensure_agent_connected().expect("ensure_agent_connected");
        println!(
            "[LIVE CONNECT] pid={:?}, client_id={}, state_code={}, evts_on_connect={}",
            conn_st.agent_pid,
            conn_st.client_id,
            conn_st.state_code,
            playback_event_count() - evt_before
        );

        struct AgentCleanupGuard;
        impl Drop for AgentCleanupGuard {
            fn drop(&mut self) {
                let _ = am_light_lib::am_agent::control("stop", None);
                am_light_lib::audio_viz::set_viz_subscribed(false);
                let _ = am_light_lib::am_agent::disconnect_agent();
            }
        }
        let _cleanup_guard = AgentCleanupGuard;

        set_viz_subscribed(true);
        let init_st = play_tracks(vec![track], 0).expect("play_tracks via AMPLibraryAgent");
        let (hr_pb, hr_pbcb, reg_toks) = am_light_lib::am_agent::playback_reg_diagnostics();
        println!(
            "[LIVE] Initial state after play_tracks: status={}, dur={:.1}s | hr_pb=0x{:08X}, hr_pbcb=0x{:08X}, toks=0x{:016X}",
            init_st.status,
            init_st.duration_sec,
            hr_pb as u32,
            hr_pbcb as u32,
            reg_toks
        );

        let mut viz = VisualizerState::default();
        viz.style = VizStyle::ChernobylRods;
        let mut max_rod_seen = 0.0f32;
        let mut mean_rod_seen = 0.0f32;

        for sec_idx in 0..420 {
            std::thread::sleep(std::time::Duration::from_millis(16));
            let snap = sample_live_visualizer(0.016);
            viz.step(&snap, 0.016, false);
            let (rm, rx, rs) = viz.rod_lift_stats();
            if snap.active_signal {
                max_rod_seen = max_rod_seen.max(rx);
                mean_rod_seen = mean_rod_seen.max(rm);
            }
            if sec_idx % 60 == 59 {
                let pb = get_playback_state();
                let evts = playback_event_count();
                let apps_dbg: Vec<String> = snap
                    .active_apps
                    .iter()
                    .map(|a| format!("{}:{:.1}dB", a.exe_name, a.peak_db))
                    .collect();
                println!(
                    "[LIVE t={:.1}s] pb_evts={} status={} pos={:.2}/{:.1}s qual={:?} apps={:?} | rms={:.1}dB tier={} title='{}' cut={:.2}kHz drop={:.1}dB bw={} grid='{}' | rods(mean={:.1}px, max={:.1}px, std={:.1}px)",
                    (sec_idx + 1) as f32 * 0.016,
                    evts,
                    pb.status,
                    pb.position_sec,
                    pb.duration_sec,
                    pb.audio_quality,
                    apps_dbg,
                    snap.rms_db,
                    snap.tier_code,
                    snap.verdict_title,
                    snap.cutoff_hz / 1000.0,
                    snap.brickwall_drop_db,
                    snap.has_brickwall_cut,
                    snap.detected_bit_grid,
                    rm,
                    rx,
                    rs
                );
                let other_louder_app = snap.active_apps.iter().any(|a| {
                    !a.exe_name.eq_ignore_ascii_case("AMPLibraryAgent.exe") && a.peak_db > -45.0
                });
                if !other_louder_app {
                    assert!(
                        !snap.tier_code.starts_with("lossy"),
                        "LIVE FALSE POSITIVE DETECTED on 下一个天亮: tier={}, title={}",
                        snap.tier_code,
                        snap.verdict_title
                    );
                }
            }
        }

        let evt_after = playback_event_count();
        let final_pb = get_playback_state();

        // 3. Attack Surface 2: Kill AMPLibraryAgent.exe externally mid-song and verify seamless
        // session reconnect + fresh IPlaybackEventDelegate callback delivery on `control("toggle", None)`!
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/IM", "AMPLibraryAgent.exe"])
            .output();
        std::thread::sleep(std::time::Duration::from_millis(350));
        let evt_pre_recover = playback_event_count();
        let recovered_st = control("toggle", None).expect("seamless recovery after AMPLibraryAgent crash");
        assert_eq!(recovered_st.status, "Playing");
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let evt_post_recover = playback_event_count();
        println!(
            "[LIVE CRASH RECOVERY] status={}, pos={:.2}s, new_pb_evts={}",
            recovered_st.status,
            get_playback_state().position_sec,
            evt_post_recover - evt_pre_recover
        );
        assert!(
            evt_post_recover > evt_pre_recover,
            "reconnected AMPLibraryAgent session must deliver fresh IPlaybackEventDelegate callbacks"
        );

        let _ = control("stop", None);
        set_viz_subscribed(false);
        let _ = disconnect_agent();

        println!(
            "[LIVE SUMMARY] pb_events_delta={}, final_pos={:.2}s, max_rod_lift={:.1}px, mean_rod_lift={:.1}px",
            evt_after - evt_before,
            final_pb.position_sec,
            max_rod_seen,
            mean_rod_seen
        );
        assert!(
            evt_after > evt_before,
            "AMPLibraryAgent out-of-proc IPlaybackEventDelegate must deliver >0 events (got 0)"
        );
        assert!(
            final_pb.position_sec > 2.0,
            "Playback timeline must advance during live AMPLibraryAgent playback (got {:.2}s)",
            final_pb.position_sec
        );
        assert!(
            max_rod_seen >= 10.0,
            "Chernobyl reactor rods must lift >=10px on live 下一个天亮 intro (got {:.1}px)",
            max_rod_seen
        );
    }

    #[test]
    fn test_tempo_tracker_comb_autocorr_bpm_accuracy() {
        use am_light_lib::audio_viz::TempoTracker;

        let sr = 48_000u32;
        for &target_bpm in &[96.0f32, 124.0, 148.0] {
            let mut tracker = TempoTracker::new(sr);
            let beat_period = 60.0 / (target_bpm as f64);
            let total_sec = 9.0f64;
            let total_frames = (total_sec * sr as f64) as usize;
            let mut pcm: Vec<[f32; 2]> = Vec::with_capacity(total_frames);

            // Synthesize realistic multi-band drum groove:
            // - Beats 0, 1, 2, 3: 62 Hz sub-bass kick + transient click
            // - Beats 1, 3: 220 Hz + broadband snare burst
            // - Off-beats (0.5, 1.5, ...): 6.5 kHz hi-hat (tests octave/double-time disambiguation!)
            for n in 0..total_frames {
                let t = (n as f64) / (sr as f64);
                let beat_pos = t / beat_period;
                let beat_idx = beat_pos.floor() as u64;
                let within_beat = (beat_pos - beat_pos.floor()) * beat_period;

                // Kick on every quarter-note beat
                let kick_env = (-within_beat * 26.0).exp() as f32;
                let kick_freq = 58.0 + 90.0 * (-within_beat * 55.0).exp();
                let kick = (2.0 * std::f64::consts::PI * kick_freq * within_beat).sin() as f32 * kick_env * 0.65;

                // Snare on backbeats (1, 3)
                let snare = if beat_idx % 2 == 1 {
                    let s_env = (-within_beat * 20.0).exp() as f32;
                    let noise = (((n as u64).wrapping_mul(6364136223846793005).wrapping_add(1) >> 33) as f32
                        / (u32::MAX as f32 / 2.0))
                        - 1.0;
                    let body = (2.0 * std::f64::consts::PI * 210.0 * within_beat).sin() as f32;
                    (noise * 0.55 + body * 0.45) * s_env * 0.42
                } else {
                    0.0
                };

                // 8th-note hi-hat
                let eighth_within = ((beat_pos * 2.0).fract()) * (beat_period * 0.5);
                let hat_env = (-eighth_within * 65.0).exp() as f32;
                let hat = (2.0 * std::f64::consts::PI * 6800.0 * t).sin() as f32 * hat_env * 0.14;

                let s = (kick + snare + hat).clamp(-0.98, 0.98);
                pcm.push([s, s]);
            }

            // Feed in realistic 480-frame (10ms) WASAPI chunks
            let mut beat_triggers = 0u32;
            let mut prev_seq = 0u64;
            for chunk in pcm.chunks(480) {
                tracker.observe_stereo_pcm(chunk);
                if tracker.beat_seq != prev_seq {
                    beat_triggers += 1;
                    prev_seq = tracker.beat_seq;
                }
            }

            let err = (tracker.locked_bpm - target_bpm).abs();
            assert!(
                err <= 2.0,
                "TempoTracker failed on {target_bpm} BPM: got {:.2} BPM (err={err:.2}, conf={:.2})",
                tracker.locked_bpm,
                tracker.confidence
            );
            assert!(
                beat_triggers >= 6,
                "TempoTracker PLL must fire beat triggers along {target_bpm} BPM groove (got {beat_triggers})"
            );
        }
    }

    #[test]
    fn test_per_char_lyrics_instrumental_centering_phosphor_and_dense_reactor() {
        use am_light_lib::am_api::artwork_canonical_stem;
        use am_light_lib::audio_viz::LiveVisualizerSnapshot;
        use am_light_lib::lyrics::{
            is_effectively_instrumental_or_empty, parse_lrc_lyrics, parse_ttml_lyrics,
            parse_yrc_lyrics, LyricsAnimator,
        };
        use am_light_lib::lyrics_view::LyricsViewState;
        use am_light_lib::visualizers::VisualizerState;

        // 1. Per-character (`字`) splitting on multi-char YRC & English TTML words
        let yrc = "[1000,2400](1000,600,0)温(1600,1800,0)柔的星空\n";
        let pyrc = parse_yrc_lyrics(yrc, None).expect("parse yrc");
        assert_eq!(pyrc.lines[0].syllables.len(), 2);
        assert_eq!(
            pyrc.lines[0].char_units.len(),
            5,
            "multi-char YRC span '柔的星空' must subdivide into 4 individual character units (+ '温' = 5)"
        );
        assert_eq!(pyrc.lines[0].char_units[0].ch_str, "温");
        assert_eq!(pyrc.lines[0].char_units[1].ch_str, "柔");
        assert_eq!(pyrc.lines[0].char_units[4].ch_str, "空");
        for w in pyrc.lines[0].char_units.windows(2) {
            assert!(w[1].start_sec > w[0].start_sec);
        }

        // Verify character lifts trigger sequentially one character at a time!
        let mut anim = LyricsAnimator::default();
        anim.bind_lyrics("test:char", &pyrc);
        for _ in 0..12 {
            anim.step(&pyrc, 1.85, 800.0, 1.0 / 60.0); // mid-way: '温' and '柔' started, '空' not yet started!
        }
        assert!(anim.line_states[0].char_lifts[0].value < -0.5);
        assert!(anim.line_states[0].char_lifts[1].value < -0.3);
        assert_eq!(
            anim.line_states[0].char_lifts[4].value, 0.0,
            "unreached trailing character '空' must stay at 0.0 lift while earlier characters float up!"
        );

        // 2. Monotonic interlude blur transition (zero sign-reversal / flicker)
        let ttml_gap = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
            <p begin="00:01.000" end="00:03.000"><span begin="00:01.000" end="00:03.000">First line</span></p>
            <p begin="00:14.000" end="00:17.000"><span begin="00:14.000" end="00:17.000">After interlude</span></p>
        </div></body></tt>"#;
        let p_gap = parse_ttml_lyrics(ttml_gap, "TTML").expect("ttml");
        let mut g_anim = LyricsAnimator::default();
        g_anim.bind_lyrics("test:gap", &p_gap);
        for _ in 0..20 {
            g_anim.step(&p_gap, 2.0, 800.0, 1.0 / 60.0);
        }
        assert!(g_anim.line_states[0].blur_spring.value <= 0.01);
        let mut prev_blur = g_anim.line_states[0].blur_spring.value;
        for f in 0..60 {
            let t = 3.30 + (f as f64) * (1.0 / 60.0);
            g_anim.step(&p_gap, t, 800.0, 1.0 / 60.0);
            let cur_blur = g_anim.line_states[0].blur_spring.value;
            assert!(
                cur_blur + 1e-4 >= prev_blur,
                "interlude blur must increase monotonically without ringing/flicker: prev={prev_blur}, cur={cur_blur}"
            );
            prev_blur = cur_blur;
        }
        assert!(prev_blur > 1.5);

        // 3. Smart Instrumental / No-Lyrics classifier + Nonlinear Spring Centering
        assert!(is_effectively_instrumental_or_empty(None));
        let inst_1 = parse_lrc_lyrics("[00:00.00]纯音乐，请欣赏\n", None, "LRC");
        assert!(is_effectively_instrumental_or_empty(inst_1.as_ref()));
        let inst_2 = parse_lrc_lyrics(
            "[00:00.00]Summer - Joe Hisaishi\n[00:01.00]作曲 : 久石让\n[00:02.00]纯音乐 请欣赏\n",
            None,
            "LRC",
        );
        assert!(is_effectively_instrumental_or_empty(inst_2.as_ref()));
        let inst_3 = parse_lrc_lyrics(
            "[00:00.00]作曲 : 坂本龙一\n[00:02.00]编曲 : 坂本龙一\n",
            None,
            "LRC",
        );
        assert!(is_effectively_instrumental_or_empty(inst_3.as_ref()));
        let inst_4 = parse_lrc_lyrics("[00:00.50]Instrumental\n", None, "LRC");
        assert!(is_effectively_instrumental_or_empty(inst_4.as_ref()));
        assert!(!is_effectively_instrumental_or_empty(Some(&p_gap)));

        let mut lv = LyricsViewState::default();
        for _ in 0..60 {
            lv.step(inst_2.as_ref(), false, "inst", 1.0, true, 0.2, None, 800.0, 1.0 / 60.0, false, true);
        }
        assert!(
            lv.center_layout_spring.value > 0.98,
            "instrumental track must smoothly center cover & controls (got {})",
            lv.center_layout_spring.value
        );
        for _ in 0..60 {
            lv.step(Some(&p_gap), false, "vocal", 1.5, true, 0.2, None, 800.0, 1.0 / 60.0, false, true);
        }
        assert!(
            lv.center_layout_spring.value < 0.02,
            "switching back to vocal track must smoothly glide cover back left (got {})",
            lv.center_layout_spring.value
        );

        // 4. Canonical artwork stem unifies multi-resolution Apple CDN URLs (eliminates reload black flash!)
        let u240 = "https://is1-ssl.mzstatic.com/image/thumb/Music126/v4/aa/bb/cover.jpg/240x240bb.jpg";
        let u600 = "https://is1-ssl.mzstatic.com/image/thumb/Music126/v4/aa/bb/cover.jpg/600x600bb.jpg";
        assert_eq!(artwork_canonical_stem(u240), artwork_canonical_stem(u600));

        // 5. Dense 37x37 (1,369) Reactor Matrix + Fixed-Coordinate Phosphor Soundstage
        let mut viz = VisualizerState::default();
        assert_eq!(viz.matrix_pillar_count(), 37 * 37);

        let mut snap1 = LiveVisualizerSnapshot {
            active_signal: true,
            rms_db: -20.0,
            peak_db_l: -16.0,
            peak_db_r: -16.0,
            mid_energy: 0.25,
            soundstage_lr: vec![[0.25, -0.15]; 32],
            ..LiveVisualizerSnapshot::default()
        };
        snap1.spectrum_64[18] = 0.30;
        viz.step(&snap1, 1.0 / 60.0, false);
        let first_dots = viz.phosphor_dots_snapshot();
        assert!(!first_dots.is_empty());
        let (sx0, sy0, life0) = first_dots[0];

        // Step next frame with much louder completely different L/R signal — existing dot MUST stay at (sx0, sy0)!
        let mut snap2 = snap1.clone();
        snap2.soundstage_lr = vec![[-0.85, 0.90]; 32];
        viz.step(&snap2, 1.0 / 60.0, false);
        let second_dots = viz.phosphor_dots_snapshot();
        let (sx1, sy1, life1) = second_dots[0];
        assert_eq!(sx0, sx1, "phosphor dot X must remain fixed at birth coordinate");
        assert_eq!(sy0, sy1, "phosphor dot Y must remain fixed at birth coordinate");
        assert!(life1 < life0, "phosphor dot luminescence must decay smoothly over time");
    }

    #[test]
    fn test_liquid_glass_default_off_cjk_vs_word_granularity_and_audio_lyrics_sync() {
        use am_light_lib::am_agent::{
            dispatch_playback_event_plist, get_playback_state, notify_optimistic_seek,
        };
        use am_light_lib::app_ui::{AppSettings, AppUi, UiAction};
        use am_light_lib::bplist::PlistValue;
        use am_light_lib::lyrics::{has_mismatched_edition_tag, is_cjk_char, parse_ttml_lyrics};

        // 1. Rebuilt iOS 26 Liquid Glass Default ON + English-by-Word Default ON (even on legacy settings_v2.json!)
        let def_s = AppSettings::default();
        assert!(
            def_s.liquid_glass_enabled,
            "Rebuilt iOS 26 Liquid Glass must be enabled by default"
        );
        assert!(
            def_s.lyrics_latin_by_word,
            "English/Latin lyrics must default to word-level units"
        );

        let legacy_json = r#"{"storefront":"cn","reduce_transparency":false,"window_corner_radius":12.0}"#;
        let migrated: AppSettings = serde_json::from_str(legacy_json).expect("deserialize legacy");
        assert!(
            migrated.liquid_glass_enabled,
            "existing settings_v2.json without liquid_glass_enabled must default to ON"
        );
        assert!(
            migrated.lyrics_latin_by_word,
            "existing settings_v2.json without lyrics_latin_by_word must default to word mode ON"
        );

        let mut ui = AppUi::new(0);
        assert!(!ui.glass_off(), "AppUi::glass_off() must be false by default with Liquid Glass ON");
        ui.execute_action(UiAction::ToggleLiquidGlass);
        assert!(!ui.settings.liquid_glass_enabled && ui.glass_off());
        ui.execute_action(UiAction::ToggleLiquidGlass);
        assert!(ui.settings.liquid_glass_enabled && !ui.glass_off());

        // 2. CJK (Chinese Hanzi / Japanese Kana+Kanji / Korean Hangul) per-character (`字`) vs English per-word (`word`) + Switch
        assert!(is_cjk_char('温') && is_cjk_char('あ') && is_cjk_char('カ') && is_cjk_char('밤'));
        assert!(!is_cjk_char('A') && !is_cjk_char('é'));

        let mixed_ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
            <p begin="00:01.000" end="00:04.200"><span begin="00:01.000" end="00:01.300">beau</span><span begin="00:01.300" end="00:01.700">ti</span><span begin="00:01.700" end="00:02.100">ful </span><span begin="00:02.100" end="00:02.900">温柔的</span><span begin="00:02.900" end="00:03.500">春の</span><span begin="00:03.500" end="00:03.800">밤 </span><span begin="00:03.800" end="00:04.200">Don't stop!</span></p>
        </div></body></tt>"#;
        let parsed = parse_ttml_lyrics(mixed_ttml, "TTML").expect("mixed ttml");
        let line = &parsed.lines[0];

        // Default mode (`latin_by_word = true`): multi-syllable English merges into whole word "beautiful",
        // multi-word span "Don't stop!" splits into whole words, while Chinese/Japanese/Korean split per-character (`字`)!
        let word_cjk: Vec<&str> = line
            .units_for_mode(true)
            .iter()
            .map(|u| u.ch_str.as_str())
            .collect();
        assert_eq!(
            word_cjk,
            vec!["beautiful", "温", "柔", "的", "春", "の", "밤", "Don't", "stop!"]
        );

        // Switched mode (`latin_by_word = false`): subdivides English words into individual letters too!
        let all_chars = line.units_for_mode(false);
        assert!(all_chars.len() > word_cjk.len() + 10);
        assert_eq!(all_chars[0].ch_str, "b");
        assert_eq!(all_chars[1].ch_str, "e");

        ui.execute_action(UiAction::ToggleLyricsLatinByWord);
        assert!(!ui.settings.lyrics_latin_by_word && !ui.lyrics_view.animator.latin_by_word);
        ui.execute_action(UiAction::ToggleLyricsLatinByWord);
        assert!(ui.settings.lyrics_latin_by_word && ui.lyrics_view.animator.latin_by_word);

        // 3. Edition mismatch guard (prevents Live / DJ / Remix NetEase/LRCLIB timing desync!)
        assert!(has_mismatched_edition_tag(
            "晴天",
            "叶惠美",
            "晴天 (Live) 演唱会现场版"
        ));
        assert!(has_mismatched_edition_tag(
            "Love Story",
            "Fearless",
            "Love Story (DJ版 Remix)"
        ));
        assert!(!has_mismatched_edition_tag(
            "晴天 (Live)",
            "地表最强演唱会",
            "晴天 (Live)"
        ));

        // 4. AMPLibraryAgent Playback Clock Buffering & Zero-Drift Sync Invariants:
        // Simulate real AMPLibraryAgent buffering plist: targetRate=1.0, playButtonState="play", scrubberState="indeterminate"
        let evt_buffering = PlistValue::dict([
            (
                "nowplaying",
                PlistValue::dict([
                    ("pqiID", PlistValue::UInt(9001)),
                    ("storeAdamID", PlistValue::UInt(1440857781)),
                    ("line1", PlistValue::String("Sync Test Song".to_string())),
                ]),
            ),
            (
                "controls",
                PlistValue::dict([
                    ("playButtonState", PlistValue::String("play".to_string())),
                    ("targetRate", PlistValue::Real(1.0)),
                ]),
            ),
            (
                "time",
                PlistValue::dict([
                    (
                        "scrubberState",
                        PlistValue::String("indeterminate".to_string()),
                    ),
                    ("canBePositioned", PlistValue::Bool(false)),
                    ("currentTime", PlistValue::Real(0.0)),
                    ("duration", PlistValue::Real(215.0)),
                ]),
            ),
        ]);
        dispatch_playback_event_plist(&evt_buffering);

        // Sleep 120ms to simulate HLS / FairPlay buffering — playhead MUST stay pinned at 0.00s!
        std::thread::sleep(std::time::Duration::from_millis(120));
        let st_buf = get_playback_state();
        assert!(
            st_buf.position_sec < 0.005,
            "playhead MUST NOT advance during network buffering! got {}",
            st_buf.position_sec
        );

        // Now AMPLibraryAgent transitions to active audio (`playButtonState = "pause"`, `scrubberState = "duration"`)
        // WITHOUT sending `currentTime` — playhead MUST start cleanly at ~0.00s (never baking the 120ms buffering delay!)
        let evt_audio_start = PlistValue::dict([
            (
                "controls",
                PlistValue::dict([
                    ("playButtonState", PlistValue::String("pause".to_string())),
                    ("targetRate", PlistValue::Real(1.0)),
                ]),
            ),
            (
                "time",
                PlistValue::dict([
                    ("scrubberState", PlistValue::String("duration".to_string())),
                    ("canBePositioned", PlistValue::Bool(true)),
                ]),
            ),
        ]);
        dispatch_playback_event_plist(&evt_audio_start);
        let st_onset = get_playback_state();
        assert_eq!(st_onset.status, "Playing");
        assert!(
            st_onset.position_sec < 0.06,
            "onset must start cleanly near 0.00s without baking buffering delay! got {}",
            st_onset.position_sec
        );

        // Optimistic seek must immediately lock new position and reject stale pre-seek COM callbacks!
        notify_optimistic_seek(72.5);
        assert!((get_playback_state().position_sec - 72.5).abs() < 0.08);
        dispatch_playback_event_plist(&PlistValue::dict([(
            "time",
            PlistValue::dict([
                ("canBePositioned", PlistValue::Bool(true)),
                ("currentTime", PlistValue::Real(0.15)), // stale pre-seek packet!
            ]),
        )]));
        assert!(
            (get_playback_state().position_sec - 72.5).abs() < 0.15,
            "stale pre-seek callback must not rubber-band playhead backward!"
        );

        // Reset clean Stopped state
        dispatch_playback_event_plist(&PlistValue::dict([
            ("currentTime", PlistValue::Real(0.0)),
            ("isPlaying", PlistValue::Bool(false)),
        ]));
    }

    #[test]
    fn test_sidebar_spring_animations_and_fast_rhythm_sampler_perf() {
        use am_light_lib::app_ui::{AppUi, UiAction};
        use am_light_lib::audio_viz::sample_rhythm_levels_fast;
        use std::time::Instant;

        let mut ui = AppUi::new(0);
        ui.settings.sidebar_collapsed = false;
        ui.sidebar_open_spring.snap(1.0);
        ui.route_trans = 1.0;
        assert!(!ui.has_active_ui_motion());

        // 1. Sidebar collapse spring (`1.0 -> 0.0`) must animate smoothly without overshoot, then settle to 0% idle!
        ui.execute_action(UiAction::ToggleSidebar);
        assert!(ui.settings.sidebar_collapsed);
        assert!(ui.has_active_ui_motion() && ui.needs_continuous_animation());

        let mut prev_v = ui.sidebar_open_spring.value;
        for _ in 0..45 {
            ui.sidebar_open_spring.step(1.0 / 120.0);
            let cur = ui.sidebar_open_spring.value;
            assert!(
                cur <= prev_v + 1e-5 && cur >= -0.01,
                "sidebar collapse spring must glide monotonically without wobble: {prev_v} -> {cur}"
            );
            prev_v = cur;
        }
        assert!(prev_v < 0.01);
        for _ in 0..20 {
            ui.sidebar_open_spring.step(1.0 / 120.0);
        }
        assert!(ui.sidebar_open_spring.is_settled_norm());
        assert!(!ui.has_active_ui_motion());

        // 2. Sidebar section accordion springs (`资料库`, `播放列表`) & `编辑` mode spring
        ui.execute_action(UiAction::ToggleSectionLibrary);
        ui.execute_action(UiAction::ToggleSectionPlaylists);
        ui.execute_action(UiAction::ToggleSidebarEdit);
        assert!(ui.has_active_ui_motion());
        for _ in 0..65 {
            ui.sec_library_spring.step(1.0 / 120.0);
            ui.sec_playlists_spring.step(1.0 / 120.0);
            ui.sidebar_edit_spring.step(1.0 / 120.0);
        }
        assert!(ui.sec_library_spring.value < 0.01);
        assert!(ui.sec_playlists_spring.value < 0.01);
        assert!(ui.sidebar_edit_spring.value > 0.99);
        assert!(!ui.has_active_ui_motion());

        // 3. Fast zero-heap-allocation rhythm sampler benchmark (< 80 microseconds per call!)
        let _ = sample_rhythm_levels_fast(1.0 / 120.0);
        let t0 = Instant::now();
        for _ in 0..200 {
            let _ = sample_rhythm_levels_fast(1.0 / 120.0);
        }
        let avg_us = t0.elapsed().as_secs_f64() * 1_000_000.0 / 200.0;
        assert!(
            avg_us < 120.0,
            "sample_rhythm_levels_fast must be ultra-fast (< 120us), got {avg_us:.2}us"
        );
    }

    #[test]
    fn test_clock_fixes_fast_binaural_onset_soft_pll_alias_epoch_and_smtc() {
        use am_light_lib::am_agent::{
            current_play_cmd_epoch, dispatch_playback_event_plist, get_live_agent_playhead_ext,
            get_playback_state, state_seq, AgentTrackInput,
        };
        use am_light_lib::app_ui::AppUi;
        use am_light_lib::binaural_dsp::fast_binaural_latency_sec;
        use am_light_lib::bplist::PlistValue;
        use am_light_lib::lyrics_view::{LyricsHitAction, LyricsViewState};
        use am_light_lib::smtc::SmtcState;
        use std::time::{Duration, Instant};

        // 1. P0-1 & P2-1: Lock-free `fast_binaural_latency_sec()` (< 0.1 us/call, 0 COM, 0 locks)
        //    and zero pause/resume jump in `LyricsViewState::synced_playhead_sec`!
        let t_lat = Instant::now();
        let mut lat_sum = 0.0f64;
        for _ in 0..1000 {
            lat_sum += fast_binaural_latency_sec();
        }
        let lat_total_us = t_lat.elapsed().as_secs_f64() * 1_000_000.0;
        assert!(
            lat_sum >= 0.0 && lat_total_us < 500.0,
            "1000 calls to fast_binaural_latency_sec must take <500us (got {lat_total_us:.1}us)"
        );

        let mut lv = LyricsViewState::default();
        lv.step(None, false, "test:pause_jump", 42.50, true, 0.0, None, 720.0, 0.016, false, true);
        let ph_playing = lv.synced_playhead_sec;
        lv.step(None, false, "test:pause_jump", 42.50, false, 0.0, None, 720.0, 0.016, false, true);
        let ph_paused = lv.synced_playhead_sec;
        assert!(
            (ph_playing - ph_paused).abs() < 1e-9,
            "synced_playhead_sec must NEVER jump on pause/resume: playing={ph_playing}, paused={ph_paused}"
        );

        // 2. P0-2, P0-3, P1-1 & P1-2:
        //    - Pre-establish Song A (`pqiID = 9400`) so `pqi_switched` triggers cleanly on Song B (`pqiID = 9401`)
        dispatch_playback_event_plist(&PlistValue::dict([
            (
                "nowplaying",
                PlistValue::dict([
                    ("pqiID", PlistValue::UInt(9400)),
                    (
                        "identifiers",
                        PlistValue::dict([("storeAdamID", PlistValue::UInt(1000001))]),
                    ),
                    ("line1", PlistValue::String("Prev Track".to_string())),
                ]),
            ),
            (
                "time",
                PlistValue::dict([
                    ("scrubberState", PlistValue::String("duration".to_string())),
                    ("canBePositioned", PlistValue::Bool(true)),
                    ("currentTime", PlistValue::Real(85.0)),
                    ("duration", PlistValue::Real(269.0)),
                ]),
            ),
            (
                "controls",
                PlistValue::dict([("playButtonState", PlistValue::String("pause".to_string()))]),
            ),
        ]));

        // Simulate user clicking a library `i.xxxx` track that resolves to numeric `storeAdamID = 1440818842`
        let mut ui = AppUi::new(0);
        let lib_track = AgentTrackInput {
            id: "i.lib_alias_01".to_string(),
            catalog_id: None,
            kind: Some("song".to_string()),
            name: "晴天".to_string(),
            artist_name: "周杰伦".to_string(),
            album_name: "叶惠美".to_string(),
            duration_sec: 269.0,
            artwork_url: None,
            audio_traits: Vec::new(),
        };
        ui.begin_optimistic_playback_loading(Some(&lib_track), None);
        // Align synthetic test epoch with current agent epoch (real `play_tracks` bumps `PLAY_CMD_EPOCH` by +1)
        ui.loading_cmd_epoch = current_play_cmd_epoch();
        assert!(ui.playback_loading);
        assert_eq!(ui.smtc.adam_id.as_deref(), Some("i.lib_alias_01"));

        // P0-3: Song B arrives with `canBePositioned = true` & `scrubberState = "duration"` at `t = 0.0`
        // while `playButtonState = "play"` (FairPlay/ALAC still buffering) — MUST NOT prematurely unlock clock!
        let seq_before = state_seq();
        dispatch_playback_event_plist(&PlistValue::dict([
            (
                "nowplaying",
                PlistValue::dict([
                    ("pqiID", PlistValue::UInt(9401)),
                    (
                        "identifiers",
                        PlistValue::dict([("storeAdamID", PlistValue::UInt(1440818842))]),
                    ),
                    ("line1", PlistValue::String("晴天".to_string())),
                ]),
            ),
            (
                "controls",
                PlistValue::dict([
                    ("playButtonState", PlistValue::String("play".to_string())),
                    ("targetRate", PlistValue::Real(1.0)),
                ]),
            ),
            (
                "time",
                PlistValue::dict([
                    ("scrubberState", PlistValue::String("duration".to_string())),
                    ("canBePositioned", PlistValue::Bool(true)),
                    ("currentTime", PlistValue::Real(0.0)),
                    ("duration", PlistValue::Real(269.0)),
                ]),
            ),
        ]));
        assert!(state_seq() > seq_before);
        std::thread::sleep(Duration::from_millis(65));
        assert!(
            get_playback_state().position_sec < 0.005,
            "P0-3: canBePositioned=true at t=0.0 with playButtonState='play' must NOT advance clock during buffering!"
        );

        // Now render clock advances past 20ms (`currentTime = 0.08`, `playButtonState = "pause"`) on resolved `storeAdamID = 1440818842`
        dispatch_playback_event_plist(&PlistValue::dict([
            (
                "controls",
                PlistValue::dict([("playButtonState", PlistValue::String("pause".to_string()))]),
            ),
            (
                "time",
                PlistValue::dict([
                    ("scrubberState", PlistValue::String("duration".to_string())),
                    ("canBePositioned", PlistValue::Bool(true)),
                    ("currentTime", PlistValue::Real(0.08)),
                    ("duration", PlistValue::Real(269.0)),
                ]),
            ),
        ]));
        let (_, _, _, _, resolved_ep) = get_live_agent_playhead_ext().expect("live agent ext");
        assert!(resolved_ep >= ui.loading_cmd_epoch);

        // P0-2: `on_playback_state_changed` MUST immediately adopt `1440818842` (zero 5.2s freeze at 0.00s!)
        ui.on_playback_state_changed(get_playback_state());
        assert_eq!(ui.smtc.adam_id.as_deref(), Some("1440818842"));
        assert!(
            !ui.playback_loading,
            "P0-2: playback_loading must clear immediately once resolved storeAdamID starts playing"
        );
        assert!(ui.smtc.position_sec >= 0.08);

        // P1-1: Delayed `LoadedTimeRangesChanged` event arriving 35ms..90ms BEHIND monotonic clock must NEVER snap playhead backward!
        dispatch_playback_event_plist(&PlistValue::dict([
            (
                "controls",
                PlistValue::dict([("playButtonState", PlistValue::String("pause".to_string()))]),
            ),
            (
                "time",
                PlistValue::dict([
                    ("scrubberState", PlistValue::String("duration".to_string())),
                    ("canBePositioned", PlistValue::Bool(true)),
                    ("currentTime", PlistValue::Real(20.00)),
                    ("duration", PlistValue::Real(269.0)),
                ]),
            ),
        ]));
        std::thread::sleep(Duration::from_millis(50));
        let pos_before_late_hls = get_playback_state().position_sec;
        // Simulate HLS chunk callback carrying a 60ms-stale `currentTime`
        dispatch_playback_event_plist(&PlistValue::dict([(
            "time",
            PlistValue::dict([
                ("scrubberState", PlistValue::String("duration".to_string())),
                ("canBePositioned", PlistValue::Bool(true)),
                ("currentTime", PlistValue::Real(pos_before_late_hls - 0.075)),
            ]),
        )]));
        let pos_after_late_hls = get_playback_state().position_sec;
        assert!(
            pos_after_late_hls + 1e-4 >= pos_before_late_hls,
            "P1-1: delayed LoadedTimeRangesChanged must NEVER snap playhead backward: before={pos_before_late_hls}, after={pos_after_late_hls}"
        );

        // P1-2 / P0-4: Gapless `pqi_switched` to an uncached track without `duration` must reset `duration_sec` to 0.0 (never keep Song A's 269.0s!)
        dispatch_playback_event_plist(&PlistValue::dict([(
            "nowplaying",
            PlistValue::dict([
                ("pqiID", PlistValue::UInt(9402)),
                ("storeAdamID", PlistValue::UInt(987654321)),
                ("line1", PlistValue::String("Uncached Gapless Song".to_string())),
            ]),
        )]));
        assert_eq!(
            get_playback_state().duration_sec,
            0.0,
            "P1-2: pqi_switched must reset duration_sec to 0.0 instead of carrying over previous song's duration!"
        );

        // Shut down active agent state (`currentTime > 0.02` clears onset watch, then `playButtonState = "stop"` -> `snap.active = false`)
        dispatch_playback_event_plist(&PlistValue::dict([
            ("currentTime", PlistValue::Real(0.05)),
            ("isPlaying", PlistValue::Bool(false)),
        ]));
        dispatch_playback_event_plist(&PlistValue::dict([(
            "controls",
            PlistValue::dict([("playButtonState", PlistValue::String("stop".to_string()))]),
        )]));
        assert!(get_live_agent_playhead_ext().is_none());

        // 3. P1-3 & P1-4: External SMTC `sampled_at`, zero-sawtooth `timeline_updated_univ`, seek snapback guard, & stale track-switch clamp!
        let mut smtc_ui = AppUi::new(0);
        {
            let base_smtc = SmtcState {
                active: true,
                app_id: "AppleInc.AppleMusicWin".to_string(),
                title: "Song Alpha".to_string(),
                artist: "Artist One".to_string(),
                album: "Album One".to_string(),
                status: "Playing".to_string(),
                position_sec: 238.2,
                duration_sec: 240.0,
                backend_mode: "smtc".to_string(),
                sampled_at: Some(Instant::now() - Duration::from_millis(80)),
                timeline_updated_univ: 500_001,
                position_is_calibrated: false,
                ..SmtcState::default()
            };
            smtc_ui.on_playback_state_changed(base_smtc.clone());
            assert!(
                smtc_ui.smtc.position_sec >= 238.27,
                "sampled_at age must be compensated on receipt"
            );
            let anchor_1 = smtc_ui.playhead_anchor_snapshot();

            // Same `timeline_updated_univ = 500_001` 40ms later MUST leave `(playhead_anchor_sec, playhead_anchor_inst)` untouched!
            std::thread::sleep(Duration::from_millis(35));
            let mut poll_same_univ = base_smtc.clone();
            poll_same_univ.sampled_at = Some(Instant::now());
            smtc_ui.on_playback_state_changed(poll_same_univ);
            let anchor_2 = smtc_ui.playhead_anchor_snapshot();
            assert_eq!(
                anchor_1, anchor_2,
                "P1-4: identical timeline_updated_univ must NEVER re-blend playhead anchor (zero 420ms sawtooth!)"
            );

            // Optimistic external SMTC seek to `60.0s` MUST suppress pre-seek `238.5s` snapshot!
            smtc_ui.handle_lyrics_action(LyricsHitAction::SeekToSec(60.0));
            let mut stale_pre_seek = base_smtc.clone();
            stale_pre_seek.position_sec = 238.6;
            stale_pre_seek.sampled_at = Some(Instant::now());
            smtc_ui.on_playback_state_changed(stale_pre_seek);
            assert!(
                (smtc_ui.smtc.position_sec - 60.0).abs() < 0.25,
                "P1-3: stale pre-seek SMTC snapshot must be suppressed, got {}",
                smtc_ui.smtc.position_sec
            );

            // Track switch to Song Beta while `timeline_updated_univ` is STILL Song Alpha's `500_001` MUST clamp position & duration to 0.0!
            let mut switch_stale_tl = base_smtc.clone();
            switch_stale_tl.title = "Song Beta".to_string();
            switch_stale_tl.position_sec = 239.4;
            switch_stale_tl.duration_sec = 240.0;
            switch_stale_tl.sampled_at = Some(Instant::now());
            smtc_ui.on_playback_state_changed(switch_stale_tl.clone());
            assert_eq!(smtc_ui.smtc.position_sec, 0.0);
            assert_eq!(smtc_ui.smtc.duration_sec, 0.0);
            assert_eq!(smtc_ui.lyrics_fetched_with_dur, 0.0);

            // Once Song Beta's real TimelineProperties (`timeline_updated_univ = 600_001`, `duration_sec = 198.0`) arrives,
            // P0-4 re-dispatches lyrics fetch with `198.0s`!
            let mut switch_fresh_tl = switch_stale_tl;
            switch_fresh_tl.timeline_updated_univ = 600_001;
            switch_fresh_tl.position_sec = 0.25;
            switch_fresh_tl.duration_sec = 198.0;
            switch_fresh_tl.sampled_at = Some(Instant::now());
            smtc_ui.on_playback_state_changed(switch_fresh_tl);
            assert!((smtc_ui.smtc.duration_sec - 198.0).abs() < 0.01);
            assert!((smtc_ui.lyrics_fetched_with_dur - 198.0).abs() < 0.01);
        }
    }

    #[test]
    fn test_lyrics_sync_fixes_end_offset_line_lrc_strict_match_yrc_ttml_and_cache() {
        use am_light_lib::app_ui::{pick_best_smtc_catalog_candidate, AppUi};
        use am_light_lib::lyrics::{
            has_mismatched_edition_tag, parse_lrc_lyrics, parse_ttml_lyrics, parse_yrc_lyrics,
            strict_catalog_match_score, LineAnimState, LyricsAnimator,
        };
        use am_light_lib::lyrics_view::{LyricsHitAction, LyricsViewState};

        // 1. P0-1 (`maxEndTimeOffset` subtraction sign):
        //    Consecutive solo lines (`1.0..4.0s`, `4.0..7.0s`) must switch `primary_active_idx` to Line 1
        //    immediately at `t_playhead = 4.00s` (NEVER holding Line 0 active for +0.50s or lighting both lines!),
        //    and unselect cleanly in silent outro (`t = 10.0s`)!
        let solo_ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
            <p begin="00:01.000" end="00:04.000"><span begin="00:01.000" end="00:04.000">First solo line</span></p>
            <p begin="00:04.000" end="00:07.000"><span begin="00:04.000" end="00:07.000">Second solo line</span></p>
        </div></body></tt>"#;
        let p_solo = parse_ttml_lyrics(solo_ttml, "TTML").expect("solo ttml");
        let mut anim = LyricsAnimator::default();
        anim.bind_lyrics("test:p0_1", &p_solo);
        anim.step(&p_solo, 2.0, 720.0, 1.0 / 60.0);
        assert_eq!(anim.primary_active_idx, Some(0));
        anim.step(&p_solo, 4.01, 720.0, 1.0 / 60.0);
        assert_eq!(
            anim.primary_active_idx,
            Some(1),
            "P0-1: Line 1 must become primary_active_idx immediately at 4.01s without +0.50s holdover on Line 0!"
        );
        assert_eq!(
            anim.secondary_active_idx, None,
            "P0-1: consecutive non-overlapping solo lines must not simultaneously highlight as a fake duet!"
        );
        anim.step(&p_solo, 10.0, 720.0, 1.0 / 60.0);
        assert_eq!(
            anim.primary_active_idx, None,
            "P0-1: finished final line must unselect cleanly during song outro"
        );
        assert!(anim.line_states[1].blur_spring.target > 0.5);

        // 2. P0-2 & `#7` Section 4:
        //    Plain line-only LRC must set `has_word_sync = false` (no fake slow-motion karaoke!),
        //    cap `end_sec = min(next_start, start + 4.0)` so >= 7s breaks trigger interlude dots,
        //    and ignore bogus community `[offset:...]` tags!
        let plain_lrc = "[offset:+3000]\n[00:02.00]Short vocal phrase\n[00:13.50]After instrumental break\n";
        let p_lrc = parse_lrc_lyrics(plain_lrc, None, "LRC").expect("plain lrc");
        assert!(
            !p_lrc.is_syllable_synced && !p_lrc.lines[0].has_word_sync,
            "P0-2: plain LRC must NOT mark has_word_sync=true or synthesize fake karaoke"
        );
        assert!(p_lrc.lines[0].char_units.is_empty());
        assert!(
            (p_lrc.lines[0].start_sec - 2.0).abs() < 1e-6,
            "community [offset:+3000] must not shift LRC timestamps"
        );
        assert!(
            (p_lrc.lines[0].end_sec - 6.0).abs() < 1e-6,
            "plain LRC end_sec must cap at start + 4.0s (got {})",
            p_lrc.lines[0].end_sec
        );
        let mut lrc_anim = LyricsAnimator::default();
        lrc_anim.bind_lyrics("test:lrc_break", &p_lrc);
        lrc_anim.step(&p_lrc, 7.5, 720.0, 1.0 / 60.0);
        assert!(
            lrc_anim.interlude.visible && lrc_anim.interlude.after_line_idx == Some(0),
            "4.0s-capped plain LRC line before 11.5s gap (7.5s break >= 7.0s) must trigger 3-dot interlude indicator"
        );

        // 3. P0-3 & P0-4: `strict_catalog_match_score` & `pick_best_smtc_catalog_candidate` (zero `.unwrap_or(&arr[0])`!)
        assert!(
            strict_catalog_match_score("晴天", "周杰伦", 269.0, "晴天 (Live)", "周杰伦", 269.0).is_none(),
            "Live edition must be hard-rejected for studio track"
        );
        assert!(
            strict_catalog_match_score("Song", "Artist", 210.0, "Song (Demo)", "Artist", 210.0).is_none(),
            "Demo edition must be hard-rejected for studio track"
        );
        assert!(
            strict_catalog_match_score("Song", "Artist", 210.0, "Song (Cover)", "Artist", 210.0).is_none(),
            "Cover edition must be hard-rejected for studio track"
        );
        assert!(
            has_mismatched_edition_tag("Song", "", "Song (Demo)"),
            "has_mismatched_edition_tag must reject genuine (Demo)"
        );
        assert!(
            has_mismatched_edition_tag("Song", "", "Song (Cover)"),
            "has_mismatched_edition_tag must reject genuine (Cover)"
        );
        assert!(
            strict_catalog_match_score("紅蓮華", "LiSA", 237.0, "紅蓮華 (Demon Slayer)", "LiSA", 237.0).is_some(),
            "'Demon Slayer' must NOT false-trigger 'demo' edition keyword!"
        );
        assert!(
            !has_mismatched_edition_tag("One More Time", "", "One More Time Discovery"),
            "album 'Discovery' must NOT false-trigger 'cover' edition keyword!"
        );
        assert!(
            strict_catalog_match_score("One More Time", "Daft Punk", 320.0, "One More Time", "Daft Punk", 320.5).is_some()
                && !has_mismatched_edition_tag("Deliver", "", "Deliver Alive"),
            "'Deliver' / 'Alive' must NOT false-trigger 'live' edition keyword!"
        );
        assert!(
            strict_catalog_match_score("Deliver", "Artist", 200.0, "Deliver (From Olive Garden)", "Artist", 200.0).is_some(),
            "'Deliver' / 'Olive' must NOT false-trigger 'live' edition keyword!"
        );
        assert!(
            strict_catalog_match_score("晴天", "周杰伦", 269.0, "晴天", "周杰伦", 242.0).is_none(),
            "27s duration mismatch (Radio/TV edit) must be hard-rejected"
        );
        assert!(
            strict_catalog_match_score("晴天", "周杰伦", 269.0, "晴天", "Unrelated Cover Band", 269.0).is_none(),
            "zero artist overlap must be hard-rejected"
        );
        assert!(
            strict_catalog_match_score("晴天 (feat. 群星)", "周杰伦", 269.0, "晴天", "周杰伦", 269.4).is_some(),
            "non-edition feat. bracket noise within 2.2s duration must match cleanly"
        );

        let catalog_arr = vec![
            json!({
                "id": "111",
                "attributes": { "name": "晴天 (Live)", "artistName": "周杰伦", "albumName": "演唱会", "durationInMillis": 269000 }
            }),
            json!({
                "id": "222",
                "attributes": { "name": "晴天", "artistName": "周杰伦", "albumName": "TV Short", "durationInMillis": 90000 }
            }),
            json!({
                "id": "1440818842",
                "attributes": { "name": "晴天", "artistName": "周杰伦", "albumName": "叶惠美", "durationInMillis": 269100 }
            }),
        ];
        let picked = pick_best_smtc_catalog_candidate(&catalog_arr, "晴天", "周杰伦", "叶惠美", 269.0)
            .expect("should pick exact studio track");
        assert_eq!(picked.get("id").and_then(|v| v.as_str()), Some("1440818842"));

        // All-mismatch array MUST return `None` instead of blindly picking `&arr[0]`!
        assert!(
            pick_best_smtc_catalog_candidate(&catalog_arr[..2], "晴天", "周杰伦", "叶惠美", 269.0).is_none(),
            "pick_best_smtc_catalog_candidate must return None when only Live / wrong-duration candidates exist"
        );

        // 4. P1-5 & P1-6: Piecewise multi-syllable English sweep, per-syllable `is_sustained`, YRC `(yeah)`, & TTML `x-roman`
        let melisma_ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata"><body><div>
            <p begin="00:01.000" data-end="99:59.000" end="00:04.200">
                <span ttm:role="x-roman" begin="00:01.000" end="00:04.200">romaji leak</span>
                <span begin="00:01.000" end="00:01.150">con</span><span begin="00:01.150" end="00:01.300">grat</span><span begin="00:01.300" end="00:01.450">u</span><span begin="00:01.450" end="00:03.200">la</span><span begin="00:03.200" end="00:03.400">tions </span><span begin="00:03.400" end="00:03.600">to</span><span begin="00:03.600" end="00:03.800">ge</span><span begin="00:03.800" end="00:04.000">ther</span>
            </p>
        </div></body></tt>"#;
        let p_mel = parse_ttml_lyrics(melisma_ttml, "TTML").expect("melisma ttml");
        assert_eq!(
            p_mel.lines[0].text, "congratulations together",
            "P1-6: nested x-roman span must be stripped and data-end attribute ignored"
        );
        assert!((p_mel.lines[0].end_sec - 4.20).abs() < 1e-6);
        let units = p_mel.lines[0].units_for_mode(true);
        assert_eq!(units.len(), 2);
        assert!(
            units[0].is_sustained,
            "'la' syllable is 1.75s >= 0.48s -> 'congratulations' must have is_sustained=true"
        );
        assert!(
            !units[1].is_sustained,
            "P1-5: 'together' has 3x 0.20s syllables (total 0.60s) -> must NOT trigger false is_sustained!"
        );
        // At t = 1.45s, "con-grat-u-" (8 of 15 chars = 53.3%) just finished -> piecewise sweep must be > 0.50 (linear would be 0.187!)
        let prog_145 = units[0].sweep_progress(1.45);
        assert!(
            prog_145 > 0.50 && prog_145 < 0.60,
            "P1-5: piecewise intra-word syllable sweep must reach 'la' at 1.45s (got {prog_145})"
        );

        let paren_yrc = "[12000,1500](12000,600,0)Oh (12600,900,0)(yeah)\n";
        let p_yrc = parse_yrc_lyrics(paren_yrc, None).expect("paren yrc");
        assert_eq!(
            p_yrc.lines[0].text, "Oh (yeah)",
            "P1-6: YRC parser must preserve parenthesized vocals like '(yeah)'"
        );

        // 5. P1-7 & `#7` `0x318f16`: Per-line monotonic clamp, `content_hash` cache busting, `Ctrl+V` / `CycleLyricsSource`, & `sync_offset_sec` preservation
        let ls = LineAnimState::new(0.0, 1, 1, 0, 0);
        assert!((ls.clamped_word_time(5.20) - 5.20).abs() < 1e-6);
        anim.step(&p_solo, 5.20, 720.0, 1.0 / 60.0);
        let clamped_jitter = anim.line_states[1].clamped_word_time(5.18);
        assert!(
            clamped_jitter >= 5.20,
            "backward micro-jitter < 0.50s must never move active word sweep backward!"
        );
        let reset_seek = anim.line_states[1].clamped_word_time(4.20);
        assert!(
            (reset_seek - 4.20).abs() < 1e-6,
            "backward seek >= 0.50s must reset clamped word time cleanly"
        );

        assert_ne!(
            p_solo.content_hash(),
            p_lrc.content_hash(),
            "ParsedLyrics::content_hash must distinguish different lyrics sources/contents"
        );

        let mut lv_off = LyricsViewState::default();
        lv_off.step(Some(&p_solo), false, "meta:晴天|周杰伦", 1.5, true, 0.0, None, 720.0, 0.016, false, true);
        lv_off.sync_offset_sec = 0.3;
        lv_off.step(Some(&p_solo), false, "id:1440818842", 1.6, true, 0.0, None, 720.0, 0.016, false, true);
        assert!(
            (lv_off.sync_offset_sec - 0.3).abs() < 1e-6,
            "P1-7: upgrading key from meta:... to id:... on the same song must preserve user sync_offset_sec!"
        );

        let mut paste_ui = AppUi::new(0);
        paste_ui.show_lyrics_overlay = true;
        assert!(paste_ui.on_paste_text("[00:01.00]Pasted Line A\n[00:05.00]Pasted Line B\n"));
        assert_eq!(
            paste_ui.current_lyrics.as_ref().map(|l| l.lines.len()),
            Some(2)
        );
        paste_ui.smtc.title = "晴天".to_string();
        paste_ui.smtc.artist = "周杰伦".to_string();
        paste_ui.handle_lyrics_action(LyricsHitAction::CycleLyricsSource);
        assert_ne!(
            paste_ui.lyrics_view.source_pref,
            am_light_lib::lyrics::LyricsSourcePref::Auto
        );
    }

    #[test]
    fn test_word_lyrics_priority_no_romaji_leak_continuous_breath_and_clock_unlock() {
        use am_light_lib::am_agent::{dispatch_playback_event_plist, get_playback_state};
        use am_light_lib::bplist::PlistValue;
        use am_light_lib::lyrics::{parse_ttml_lyrics, LyricsAnimator};
        use am_light_lib::lyrics_view::LyricsViewState;
        use std::time::Duration;

        // 1. `<transliterations>` Romaji must NEVER leak into `line.translation` when `<translations/>` is empty,
        //    whereas `<translations><translation xml:lang="zh-Hans-CN">` MUST be extracted!
        let jp_ttml_romaji_only = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" itunes:timing="Word">
            <head><metadata><iTunesMetadata><translations/><transliterations><transliteration xml:lang="ja-Latn"><text for="L1"><span begin="0.589" end="1.396">muteki no</span></text></transliteration></transliterations></iTunesMetadata></metadata></head>
            <body><div><p begin="0.589" end="1.396" itunes:key="L1"><span begin="0.589" end="1.396">無敵の</span></p></div></body>
        </tt>"#;
        let p_rom = parse_ttml_lyrics(jp_ttml_romaji_only, "Apple Music 逐词歌词").expect("jp ttml");
        assert!(p_rom.is_syllable_synced);
        assert_eq!(
            p_rom.lines[0].translation, None,
            "transliterations ja-Latn Romaji must NEVER leak into line.translation when translations is empty!"
        );

        let jp_ttml_with_zh = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" itunes:timing="Word">
            <head><metadata><iTunesMetadata><translations><translation xml:lang="en-US"><text for="L1">Invincible</text></translation><translation xml:lang="zh-Hans-CN"><text for="L1">无敌的笑容</text></translation></translations><transliterations><transliteration xml:lang="ja-Latn"><text for="L1">muteki no</text></transliteration></transliterations></iTunesMetadata></metadata></head>
            <body><div><p begin="0.589" end="1.396" itunes:key="L1"><span begin="0.589" end="1.396">無敵の</span></p></div></body>
        </tt>"#;
        let p_zh = parse_ttml_lyrics(jp_ttml_with_zh, "Apple Music 逐词歌词").expect("zh ttml");
        assert_eq!(
            p_zh.lines[0].translation.as_deref(),
            Some("无敌的笑容"),
            "zh-Hans-CN translation inside <translations> must be extracted instead of Romaji or en-US"
        );

        // 2. Line-only Apple Music TTML (`itunes:timing="Line"`) sets `is_syllable_synced = false`
        //    (labeled "Apple Music 同步歌词" as Tier-2 fallback so Tier-1 syllable TTML/YRC wins!)
        let line_only_ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" itunes:timing="Line">
            <body><div><p begin="1.000" end="4.000">Line synced only</p></div></body>
        </tt>"#;
        let p_line = parse_ttml_lyrics(line_only_ttml, "Apple Music 同步歌词").expect("line ttml");
        assert!(!p_line.is_syllable_synced && !p_line.lines[0].has_word_sync);
        assert_eq!(p_line.source_name, "Apple Music 同步歌词");

        // 3. Continuous active-line handoff across <7.0s breaths (no blurry/dark gap flash!) vs >=7.0s interlude dots
        let breath_ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
            <p begin="00:01.000" end="00:04.000"><span begin="00:01.000" end="00:04.000">Line Zero</span></p>
            <p begin="00:06.500" end="00:09.500"><span begin="00:06.500" end="00:09.500">Line One after 2.5s breath</span></p>
            <p begin="00:20.500" end="00:23.500"><span begin="00:20.500" end="00:23.500">Line Two after 11s interlude</span></p>
        </div></body></tt>"#;
        let p_br = parse_ttml_lyrics(breath_ttml, "TTML").expect("breath ttml");
        let mut br_anim = LyricsAnimator::default();
        br_anim.bind_lyrics("test:breath", &p_br);
        for step_i in 0..190 {
            let t = (step_i as f64) * 0.05; // 0.00s .. 9.45s
            br_anim.step(&p_br, t, 720.0, 0.016);
            assert!(
                br_anim.primary_active_idx.is_some() && !br_anim.interlude.visible,
                "at t={t:.2}s (<7s breath), primary_active_idx must NEVER drop to None or flash interlude!"
            );
        }
        // Check handoff point cleanly: right after 4.0s at 4.10s Line 0 still holds afterglow, at 4.50s Line 1 pre-selects cleanly
        br_anim.step(&p_br, 4.10, 720.0, 0.016);
        assert_eq!(br_anim.primary_active_idx, Some(0));
        br_anim.step(&p_br, 4.50, 720.0, 0.016);
        assert_eq!(br_anim.primary_active_idx, Some(1));
        // Across the >= 7.0s interlude (9.5s -> 20.5s), countdown dots MUST activate cleanly!
        br_anim.step(&p_br, 12.0, 720.0, 0.016);
        assert!(br_anim.interlude.visible && br_anim.interlude.after_line_idx == Some(1));

        // 4. `playButtonState == "pause"` without `scrubberState` clears `scrubber_indeterminate` and unlocks clock immediately!
        dispatch_playback_event_plist(&PlistValue::dict([
            (
                "nowplaying",
                PlistValue::dict([
                    ("pqiID", PlistValue::UInt(9501)),
                    ("storeAdamID", PlistValue::UInt(1679278167)),
                    ("line1", PlistValue::String("Clock Unlock Test".to_string())),
                ]),
            ),
            (
                "controls",
                PlistValue::dict([("playButtonState", PlistValue::String("play".to_string()))]),
            ),
            (
                "time",
                PlistValue::dict([
                    ("scrubberState", PlistValue::String("indeterminate".to_string())),
                    ("currentTime", PlistValue::Real(0.0)),
                    ("duration", PlistValue::Real(213.0)),
                ]),
            ),
        ]));
        assert!(!get_playback_state().position_is_calibrated);

        // Audio begins on cached track: `playButtonState = "pause"` arrives with NO `scrubberState` key
        dispatch_playback_event_plist(&PlistValue::dict([(
            "controls",
            PlistValue::dict([("playButtonState", PlistValue::String("pause".to_string()))]),
        )]));
        // Simulate either live audio active OR >320ms elapsed quiet intro; verify `position_is_calibrated` cleared `scrubber_indeterminate` immediately
        assert!(
            get_playback_state().position_is_calibrated
                || {
                    std::thread::sleep(Duration::from_millis(340));
                    dispatch_playback_event_plist(&PlistValue::dict([(
                        "controls",
                        PlistValue::dict([("playButtonState", PlistValue::String("pause".to_string()))]),
                    )]));
                    get_playback_state().position_is_calibrated
                },
            "playButtonState='pause' without scrubberState must clear scrubber_indeterminate without 9s freeze!"
        );
        let p0 = get_playback_state().position_sec;
        std::thread::sleep(Duration::from_millis(70));
        let p1 = get_playback_state().position_sec;
        assert!(p1 > p0 + 0.03, "unlocked clock must advance cleanly: {p0} -> {p1}");

        // Clean up agent state
        dispatch_playback_event_plist(&PlistValue::dict([(
            "controls",
            PlistValue::dict([("playButtonState", PlistValue::String("stop".to_string()))]),
        )]));

        // 5. Manual `-0.1s` `sync_offset_sec` nudge must reset `max_word_t` immediately (even while paused!) + no 0.42s loading centering!
        let mut lv = LyricsViewState::default();
        for _ in 0..65 {
            lv.step(None, true, "id:1679278167", 1.0, true, 0.1, None, 720.0, 1.0 / 60.0, false, false);
        }
        assert!(
            lv.center_layout_spring.target < 0.01 && lv.center_layout_spring.value < 0.02,
            "while lyrics_loading=true (<6.0s), cover MUST NOT jump to center after 0.42s!"
        );

        let word_ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
            <p begin="00:01.000" end="00:05.000"><span begin="00:01.000" end="00:03.000">Alpha </span><span begin="00:03.000" end="00:05.000">Beta</span></p>
        </div></body></tt>"#;
        let p_wd = parse_ttml_lyrics(word_ttml, "TTML").expect("word ttml");
        lv.step(Some(&p_wd), false, "id:1679278167", 3.50, false, 0.0, None, 720.0, 0.016, false, true);
        let max_before = lv.animator.line_states[0].max_word_t;
        assert!(max_before > 3.50);
        lv.sync_offset_sec = -0.1;
        lv.step(Some(&p_wd), false, "id:1679278167", 3.50, false, 0.0, None, 720.0, 0.016, false, true);
        let max_after = lv.animator.line_states[0].max_word_t;
        assert!(
            (max_after - (max_before - 0.1)).abs() < 0.02,
            "manual -0.1s offset nudge must immediately update active word clamp even when paused: {max_before} -> {max_after}"
        );
    }

    #[test]
    fn test_backend_clock_immediate_unlock_visualizer_sub_and_non_emph_lyrics() {
        use am_light_lib::am_agent::{dispatch_playback_event_plist, get_playback_state};
        use am_light_lib::audio_viz::{is_viz_subscribed, set_viz_subscribed};
        use am_light_lib::bplist::PlistValue;
        use am_light_lib::lyrics::{parse_ttml_lyrics, NonEmphasisLyricsEffect};
        use am_light_lib::lyrics_view::build_disjoint_unit_cells;
        use am_light_lib::theme::RectF;
        use std::time::Duration;

        // 1. Authoritative backend clock: NEVER advances on targetRate=1.0 alone, unlocks IMMEDIATELY at 0:00 on pause+duration
        dispatch_playback_event_plist(&PlistValue::dict([(
            "controls",
            PlistValue::dict([("playButtonState", PlistValue::String("stop".to_string()))]),
        )]));
        dispatch_playback_event_plist(&PlistValue::dict([
            (
                "nowplaying",
                PlistValue::dict([
                    ("pqiID", PlistValue::UInt(97711)),
                    ("storeAdamID", PlistValue::UInt(99001122)),
                    ("line1", PlistValue::String("Auth Test".to_string())),
                    ("artist", PlistValue::String("Artist".to_string())),
                    ("duration", PlistValue::Real(200.0)),
                ]),
            ),
            (
                "controls",
                PlistValue::dict([
                    ("playButtonState", PlistValue::String("play".to_string())),
                    ("targetRate", PlistValue::Real(1.0)),
                ]),
            ),
            (
                "time",
                PlistValue::dict([("scrubberState", PlistValue::String("indeterminate".to_string()))]),
            ),
        ]));
        dispatch_playback_event_plist(&PlistValue::dict([(
            "controls",
            PlistValue::dict([("targetRate", PlistValue::Real(1.0))]),
        )]));
        std::thread::sleep(Duration::from_millis(60));
        let st_wait = get_playback_state();
        assert_ne!(st_wait.status, "Playing");
        assert!(!st_wait.position_is_calibrated);
        assert!(st_wait.position_sec < 0.001);

        dispatch_playback_event_plist(&PlistValue::dict([
            (
                "nowplaying",
                PlistValue::dict([("pqiID", PlistValue::UInt(97711))]),
            ),
            (
                "controls",
                PlistValue::dict([("playButtonState", PlistValue::String("pause".to_string()))]),
            ),
            (
                "time",
                PlistValue::dict([
                    ("scrubberState", PlistValue::String("duration".to_string())),
                    ("canBePositioned", PlistValue::Bool(true)),
                    ("currentTime", PlistValue::Real(0.0)),
                ]),
            ),
        ]));
        let st_now = get_playback_state();
        assert_eq!(st_now.status, "Playing");
        assert!(st_now.position_is_calibrated, "must calibrate at 0:00 without 320ms delay");
        std::thread::sleep(Duration::from_millis(75));
        assert!(get_playback_state().position_sec >= 0.045);

        // 1b. Regression: `time`-only onset packet (`currentTime > 0.02`, `canBePositioned: true`,
        // with NO `controls` dict) promotes desired playback from `Paused` to `Playing`.
        dispatch_playback_event_plist(&PlistValue::dict([(
            "controls",
            PlistValue::dict([("playButtonState", PlistValue::String("stop".to_string()))]),
        )]));
        dispatch_playback_event_plist(&PlistValue::dict([
            (
                "nowplaying",
                PlistValue::dict([
                    ("pqiID", PlistValue::UInt(97712)),
                    ("storeAdamID", PlistValue::UInt(99001123)),
                    ("line1", PlistValue::String("Time-Only Onset Test".to_string())),
                    ("artist", PlistValue::String("Artist".to_string())),
                    ("duration", PlistValue::Real(210.0)),
                ]),
            ),
            (
                "controls",
                PlistValue::dict([
                    ("playButtonState", PlistValue::String("play".to_string())),
                    ("targetRate", PlistValue::Real(1.0)),
                ]),
            ),
            (
                "time",
                PlistValue::dict([
                    ("scrubberState", PlistValue::String("indeterminate".to_string())),
                    ("currentTime", PlistValue::Real(0.0)),
                ]),
            ),
        ]));
        let st_buf = get_playback_state();
        assert_ne!(st_buf.status, "Playing");
        assert!(st_buf.position_sec < 0.001);

        dispatch_playback_event_plist(&PlistValue::dict([(
            "time",
            PlistValue::dict([
                ("currentTime", PlistValue::Real(0.08)),
                ("canBePositioned", PlistValue::Bool(true)),
            ]),
        )]));
        let st_promoted = get_playback_state();
        assert_eq!(st_promoted.status, "Playing");
        assert!(st_promoted.position_is_calibrated);
        std::thread::sleep(Duration::from_millis(60));
        assert!(get_playback_state().position_sec > 0.11);

        dispatch_playback_event_plist(&PlistValue::dict([(
            "controls",
            PlistValue::dict([("playButtonState", PlistValue::String("stop".to_string()))]),
        )]));

        // 2. WASAPI subscription: gated by Visualizer page OR `live_eq_icon_spectrum` toggle while Playing
        set_viz_subscribed(false);
        let mut ui = am_light_lib::app_ui::AppUi::new(0);
        ui.smtc.status = "Playing".to_string();
        ui.route = am_light_lib::app_ui::PageRoute::Home;
        ui.settings.live_eq_icon_spectrum = false;
        ui.update_viz_subscription();
        assert!(
            !is_viz_subscribed(),
            "Home while Playing with live_eq_icon_spectrum=false must NOT subscribe WASAPI"
        );

        ui.settings.live_eq_icon_spectrum = true;
        ui.update_viz_subscription();
        assert!(
            is_viz_subscribed(),
            "Home while Playing with live_eq_icon_spectrum=true must subscribe lightweight WASAPI"
        );

        ui.smtc.status = "Paused".to_string();
        ui.update_viz_subscription();
        assert!(!is_viz_subscribed(), "Home while Paused must release WASAPI");

        ui.smtc.status = "Playing".to_string();
        ui.route = am_light_lib::app_ui::PageRoute::Visualizer;
        ui.show_lyrics_overlay = false;
        ui.update_viz_subscription();
        assert!(is_viz_subscribed(), "Visualizer page must subscribe WASAPI");

        ui.show_lyrics_overlay = true;
        ui.update_viz_subscription();
        assert!(!is_viz_subscribed(), "Lyrics overlay on Visualizer must pause WASAPI capture");
        set_viz_subscribed(false);

        // 3. NonEmphasisLyricsEffect default & TTML emphasis vs normal syllable classification
        let def_fx = NonEmphasisLyricsEffect::default();
        assert_eq!(def_fx, NonEmphasisLyricsEffect::None);
        assert!(!def_fx.glow() && !def_fx.scale());
        assert!(NonEmphasisLyricsEffect::GlowOnly.glow() && !NonEmphasisLyricsEffect::GlowOnly.scale());
        assert!(!NonEmphasisLyricsEffect::ScaleOnly.glow() && NonEmphasisLyricsEffect::ScaleOnly.scale());
        assert!(NonEmphasisLyricsEffect::Both.glow() && NonEmphasisLyricsEffect::Both.scale());

        let emph_ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal"><body><div>
            <p begin="00:01.000" end="00:03.500">
              <span begin="00:01.000" end="00:01.350" itunes:emphasis="true">Oh</span>
              <span begin="00:01.400" end="00:01.950">world</span>
              <span begin="00:02.000" end="00:03.300">stay</span>
            </p>
        </div></body></tt>"#;
        let parsed = parse_ttml_lyrics(emph_ttml, "TTML").expect("emph ttml");
        let syls = &parsed.lines[0].syllables;
        assert!(syls[0].is_sustained, "explicit TTML emphasis must mark is_sustained");
        assert!(!syls[1].is_sustained, "0.55s normal word must NOT glow/scale by default");
        assert!(syls[2].is_sustained, "1.30s held word must mark is_sustained");

        // 4. build_disjoint_unit_cells zero horizontal & vertical overlap
        let raw_units = vec![
            vec![RectF::new(10.0, 2.0, 10.5, 36.0)],
            vec![RectF::new(20.1, 2.0, 10.0, 36.0)],
            vec![RectF::new(10.0, 36.5, 15.0, 36.0)],
        ];
        let cells = build_disjoint_unit_cells(&raw_units);
        let c0 = cells[0][0].clip_rect;
        let c1 = cells[1][0].clip_rect;
        let c2 = cells[2][0].clip_rect;
        assert!(c0.right() <= c1.x + 1e-4, "adjacent chars must not overlap horizontally");
        assert!(c0.bottom() <= c2.y + 1e-4, "adjacent wrapped rows must not overlap vertically");
    }

    #[test]
    fn test_playlist_cache_diff_and_topbar_hit_clipping() {
        use am_light_lib::am_agent::AgentTrackInput;
        use am_light_lib::app_ui::{
            clip_hit_regions_below_topbar, compute_playlist_diff, ContainerDetailData, HitRegion,
            PlaylistDiffKind, PlaylistDiskCache, UiAction, TOP_BAR_GUARD_H,
        };
        use am_light_lib::theme::RectF;

        let mk_track = |id: &str, name: &str| AgentTrackInput {
            id: id.to_string(),
            name: name.to_string(),
            ..Default::default()
        };
        let old = vec![
            mk_track("t1", "A"),
            mk_track("t2", "B"),
            mk_track("t3", "C"),
            mk_track("t2", "B"),
        ];
        let new = vec![
            mk_track("t3", "C"),
            mk_track("t4", "D"),
            mk_track("t1", "A"),
            mk_track("t2", "B"),
        ];
        assert!(compute_playlist_diff(&old, &old).is_none());
        let diff = compute_playlist_diff(&old, &new).expect("diff present");
        assert!(diff.iter().any(|e| matches!(e.kind, PlaylistDiffKind::Removed { old_idx: 3 })));
        assert!(diff.iter().any(|e| matches!(e.kind, PlaylistDiffKind::Added { new_idx: 1 })));
        assert!(diff.iter().any(|e| matches!(e.kind, PlaylistDiffKind::KeptOrMoved { old_idx: 2, new_idx: 0 })));
        assert!(diff.iter().any(|e| matches!(e.kind, PlaylistDiffKind::KeptOrMoved { old_idx: 0, new_idx: 2 })));

        let tmp = std::env::temp_dir().join("am_light_playlist_cache_test.json");
        let mut cache = PlaylistDiskCache::default();
        cache.upsert_detail(
            &ContainerDetailData {
                id: "pl.123".to_string(),
                kind: "library-playlists".to_string(),
                title: "My Mix".to_string(),
                subtitle: "资料库".to_string(),
                description: "".to_string(),
                artwork_url: None,
                motion_video_url: None,
                tracks: new.clone(),
                related_albums: Vec::new(),
            },
            1700000000,
        );
        cache.save_to_path(&tmp);
        let reloaded = PlaylistDiskCache::load_from_path(&tmp);
        let _ = std::fs::remove_file(&tmp);
        assert_eq!(reloaded.details.get("pl.123").map(|d| d.tracks.len()), Some(4));

        let mut regions = vec![
            HitRegion { rect: RectF::new(0.0, 0.0, 300.0, 50.0), action: UiAction::SwallowClick },
            HitRegion { rect: RectF::new(0.0, 30.0, 300.0, 56.0), action: UiAction::SwallowClick },
            HitRegion { rect: RectF::new(0.0, 64.0, 300.0, 56.0), action: UiAction::SwallowClick },
        ];
        clip_hit_regions_below_topbar(&mut regions, TOP_BAR_GUARD_H);
        assert_eq!(regions.len(), 2);
        assert!((regions[0].rect.y - TOP_BAR_GUARD_H).abs() < 0.01);
        assert!((regions[0].rect.h - (86.0 - TOP_BAR_GUARD_H)).abs() < 0.01);
        assert!((regions[1].rect.y - 64.0).abs() < 0.01);
    }

    #[test]
    fn test_per_page_fps_settings_adaptive_60_72_unfocused_lyrics_and_subpixel_smooth() {
        use am_light_lib::app_ui::{
            resolve_adaptive_60_72_hz, resolve_cap_sync_and_hz, AppSettings, AppUi, FpsCap,
            PageRoute, UiAction,
        };
        use am_light_lib::lyrics::{parse_yrc_lyrics, LyricsAnimator};

        // 1. Adaptive [60/72] across standard & high-refresh monitor rates
        assert_eq!(resolve_adaptive_60_72_hz(60), 60);
        assert_eq!(resolve_adaptive_60_72_hz(72), 72);
        assert_eq!(resolve_adaptive_60_72_hz(120), 60);
        assert_eq!(resolve_adaptive_60_72_hz(143), 72);
        assert_eq!(resolve_adaptive_60_72_hz(144), 72);
        assert_eq!(resolve_adaptive_60_72_hz(180), 60);
        assert_eq!(resolve_adaptive_60_72_hz(239), 60);
        assert_eq!(resolve_adaptive_60_72_hz(240), 60);
        assert_eq!(resolve_adaptive_60_72_hz(288), 72);

        // 2. Hardware VBlank divisor selection vs software hr_timer
        assert_eq!(resolve_cap_sync_and_hz(FpsCap::Adaptive60_72, 240, 120).0, 4);
        assert_eq!(resolve_cap_sync_and_hz(FpsCap::Adaptive60_72, 240, 120).1, 60);
        assert_eq!(resolve_cap_sync_and_hz(FpsCap::Adaptive60_72, 144, 120).0, 2);
        assert_eq!(resolve_cap_sync_and_hz(FpsCap::Adaptive60_72, 144, 120).1, 72);
        assert_eq!(resolve_cap_sync_and_hz(FpsCap::VSync, 144, 120), (1, 144, None));
        assert_eq!(resolve_cap_sync_and_hz(FpsCap::Hz60, 144, 120).0, 0);
        assert_eq!(resolve_cap_sync_and_hz(FpsCap::Custom, 240, 165).1, 165);
        assert_eq!(resolve_cap_sync_and_hz(FpsCap::Custom, 240, 165).0, 0);

        // 3. Independent Visualizer & Lyrics caps + unfocused Lyrics policy
        let mut ui = AppUi::new(0);
        ui.current_monitor_hz = 240;
        ui.is_window_focused = false;
        assert!(!ui.settings.lyrics_limit_unfocused_fps, "unfocused lyrics must stay full FPS by default");

        ui.execute_action(UiAction::SetVizFpsCap(FpsCap::Hz72));
        ui.execute_action(UiAction::SetLyricsFpsCap(FpsCap::VSync));
        ui.route = PageRoute::Visualizer;
        ui.show_lyrics_overlay = false;
        assert_eq!(ui.effective_frame_pacing().1, 72);

        ui.show_lyrics_overlay = true;
        assert_eq!(ui.effective_frame_pacing(), (1, 240, None), "unfocused lyrics keeps full 240Hz by default");

        ui.execute_action(UiAction::ToggleLyricsLimitUnfocusedFps);
        assert_eq!(ui.effective_frame_pacing().1, 60, "240Hz unfocused adaptive limits to 60Hz");
        ui.current_monitor_hz = 144;
        assert_eq!(ui.effective_frame_pacing().1, 72, "144Hz unfocused adaptive limits to 72Hz");
        ui.is_window_focused = true;
        assert_eq!(ui.effective_frame_pacing(), (1, 144, None), "refocused lyrics restores 144Hz VSync");

        // Respect stricter user cap (e.g. Custom 45Hz) even when unfocused adaptive limit is on
        ui.execute_action(UiAction::SetLyricsCustomFps(45));
        ui.is_window_focused = false;
        assert_eq!(ui.effective_frame_pacing().1, 45);

        // 4. Legacy settings JSON deserialization preserves token and migrates removed Hz variants
        let legacy_json = r#"{"viz_fps_cap":"Hz144","user_token":"abc123token"}"#;
        let parsed_settings: AppSettings = serde_json::from_str(legacy_json).expect("legacy json");
        assert_eq!(parsed_settings.viz_fps_cap, FpsCap::Custom);
        assert_eq!(parsed_settings.user_token, "abc123token");

        // 5. Monotonic smoothstep float-up onset from 0.0 -> -CHAR_LIFT_PX
        let yrc_sample = "[1000,1000](1000,400,0)你(1400,400,0)好\n";
        let pyrc = parse_yrc_lyrics(yrc_sample, None).expect("parse yrc");
        let mut anim = LyricsAnimator::default();
        anim.bind_lyrics("test:smooth", &pyrc);
        let mut prev_lift = 0.0f32;
        for step_i in 0..16 {
            let t = 0.90 + (step_i as f64) * 0.015;
            anim.step(&pyrc, t, 800.0, 1.0 / 60.0);
            let cur = anim.line_states[0].char_lifts[0].value;
            assert!(cur <= prev_lift + 1e-4, "lift must move smoothly upward without bounce at step {step_i}: cur={cur} prev={prev_lift}");
            prev_lift = cur;
        }
        assert!(prev_lift < -0.5);
    }

    #[test]
    fn test_apple_music_drm_diagnosis_healthy_readonly_and_redirections() {
        use am_light_lib::drm_fix::diagnose_drm_paths;

        let uniq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("am_light_drm_test_{uniq}"));
        let adi_dir = root.join("adi");
        let sc_dir = root.join("SC Info");
        let sc_file = sc_dir.join("SC Info.txt");
        let _ = std::fs::remove_dir_all(&root);

        // 1. Missing folder -> healthy initial state
        let rep0 = diagnose_drm_paths(&adi_dir, &sc_file, Vec::new());
        assert!(rep0.checked && rep0.healthy && !rep0.adi_dir_exists && rep0.adi_file_count == 0);

        // 2. Writable ADI file -> healthy
        std::fs::create_dir_all(&adi_dir).unwrap();
        std::fs::create_dir_all(&sc_dir).unwrap();
        let adi_pb = adi_dir.join("adi.pb");
        std::fs::write(&adi_pb, b"test-adi").unwrap();
        let rep1 = diagnose_drm_paths(&adi_dir, &sc_file, Vec::new());
        assert!(rep1.healthy && rep1.adi_dir_exists && rep1.adi_file_count == 1);
        assert!(rep1.unwritable_files.is_empty());

        // 3. Read-only ADI file -> detected as unhealthy
        let mut perms = std::fs::metadata(&adi_pb).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&adi_pb, perms).unwrap();
        let rep2 = diagnose_drm_paths(&adi_dir, &sc_file, Vec::new());
        assert!(!rep2.healthy);
        assert_eq!(rep2.unwritable_files, vec!["adi.pb".to_string()]);

        // Restore write bit for clean removal & test redirection warning
        let mut perms2 = std::fs::metadata(&adi_pb).unwrap().permissions();
        perms2.set_readonly(false);
        std::fs::set_permissions(&adi_pb, perms2).unwrap();
        let rep3 = diagnose_drm_paths(
            &adi_dir,
            &sc_file,
            vec!["LibSidePath = C:\\Sideloadly\\CoreADI.dll".to_string()],
        );
        assert!(rep3.healthy);
        assert_eq!(rep3.hklm_redirections.len(), 1);
        assert!(rep3.summary.contains("第三方 DRM 重定向"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_soundstage_semicircle_folding() {
        use am_light_lib::visualizers::map_lr_to_semicircle;

        for (l, r, g) in [
            (0.5f32, 0.5f32, 1.5f32),
            (-0.6, -0.6, 1.8),
            (0.9, -0.9, 2.2),
            (-0.8, 0.2, 1.4),
            (0.0, 0.0, 1.0),
        ] {
            let (nx, ny) = map_lr_to_semicircle(l, r, g);
            assert!(ny >= 0.0, "semicircle ny must never drop below 0: {ny}");
            assert!(
                nx * nx + ny * ny <= 1.0 + 1e-4,
                "semicircle magnitude must stay within unit disk: ({nx}, {ny})"
            );
        }

        // Pure mono in both positive & negative half-cycles maps straight up X = 0
        let (mx_pos, my_pos) = map_lr_to_semicircle(0.4, 0.4, 1.2);
        let (mx_neg, my_neg) = map_lr_to_semicircle(-0.4, -0.4, 1.2);
        assert!(mx_pos.abs() < 1e-5 && my_pos > 0.3);
        assert!(mx_neg.abs() < 1e-5 && (my_pos - my_neg).abs() < 1e-5);

        // Mirror stereo panning symmetry
        let (lx, ly) = map_lr_to_semicircle(0.6, 0.0, 1.2);
        let (rx, ry) = map_lr_to_semicircle(0.0, 0.6, 1.2);
        assert!(lx < 0.0 && rx > 0.0);
        assert!((lx + rx).abs() < 1e-5 && (ly - ry).abs() < 1e-5);
    }

    #[test]
    fn test_lyrics_auto_emphasis_toggle() {
        use am_light_lib::lyrics::parse_ttml_lyrics;

        // One word has explicit `emphasis="true"`, another is long sustained duration without explicit tag
        let ttml = r#"<?xml version="1.0" encoding="UTF-8"?>
<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">
  <body><div>
    <p begin="1.0s" end="4.5s">
      <span begin="1.0s" end="2.4s">天</span>
      <span begin="2.5s" end="4.0s" ttm:role="x-emphasis">亮</span>
    </p>
  </div></body>
</tt>"#;
        let parsed = parse_ttml_lyrics(ttml, "Apple Music TTML").expect("parse ttml");
        let units = &parsed.lines[0].word_cjk_units;
        assert_eq!(units.len(), 2);
        assert!(!units[0].is_explicit_emphasis, "untagged long syllable must NOT be explicit");
        assert!(units[0].is_sustained, "long 1.4s syllable IS auto-detected as sustained");
        assert!(units[0].is_emphasized(true), "auto_detect=true enables heuristic emphasis");
        assert!(
            !units[0].is_emphasized(false),
            "auto_detect=false must reject heuristic-only emphasis"
        );
        assert!(units[1].is_explicit_emphasis, "explicit span must set is_explicit_emphasis");
        assert!(units[1].is_emphasized(false), "auto_detect=false must still allow explicit emphasis");
    }

    #[test]
    fn test_lyrics_frontier_lift_scale_glow_and_speed_gate() {
        use am_light_lib::lyrics::{
            eval_frontier_glow_profile, eval_frontier_lift_weight, eval_frontier_scale_delta,
            should_use_frontier_lift,
        };

        // 1. Speed gate for non-emphasized frontier lift
        assert!(
            !should_use_frontier_lift(false, 0.25, 110.0),
            "fast non-emphasized word must skip frontier split for performance"
        );
        assert!(
            !should_use_frontier_lift(false, 0.60, 140.0),
            "233 DIP/s non-emphasized word is faster than slow threshold -> skip"
        );
        assert!(
            should_use_frontier_lift(false, 0.90, 90.0),
            "slow 0.90s @ 100 DIP/s non-emphasized word must use frontier lift"
        );
        assert!(
            should_use_frontier_lift(true, 0.22, 120.0),
            "emphasized unit always uses frontier mechanics regardless of speed"
        );

        // 2. Frontier lift progresses cleanly ahead/behind progress boundary
        let back_l = eval_frontier_lift_weight(0.15, 0.50, false);
        let front_l = eval_frontier_lift_weight(0.85, 0.50, false);
        assert!(back_l > 0.85 && front_l < 0.15, "back={back_l}, front={front_l}");

        // 3. Frontier zoom bulge peaks near frontier and tapers away
        let peak_d = eval_frontier_scale_delta(0.50, 0.50, 0.14, false);
        let edge_d = eval_frontier_scale_delta(0.05, 0.50, 0.14, false);
        assert!(peak_d > 0.12 && edge_d < peak_d * 0.4, "peak={peak_d}, edge={edge_d}");

        // 4. Glow profile: legible luminous swept body (never blinding white), leading frontier peak, smooth non-abrupt end-hold rise
        let (sweep_body, sweep_peak, _) = eval_frontier_glow_profile(0.50, 0.0, 0.0, true, true, 0.8);
        assert!(
            sweep_body > 0.40 && sweep_body < 0.62 && sweep_peak > sweep_body * 1.25 && sweep_peak <= 0.85,
            "swept body {sweep_body} should be legible & lit while frontier peak {sweep_peak} leads clearly without whiteout"
        );

        let (near_end_body, _, _) = eval_frontier_glow_profile(0.98, 0.0, 0.0, true, true, 0.8);
        let (hold_early, hold_peak0, _) = eval_frontier_glow_profile(1.0, 0.02, 0.0, true, true, 0.8);
        let (hold_late, hold_peak1, _) = eval_frontier_glow_profile(1.0, 0.45, 0.0, true, true, 0.8);
        assert!(
            (hold_early - near_end_body).abs() < 0.14,
            "end-hold entry must be smooth, not abrupt: near_end={near_end_body}, hold_early={hold_early}"
        );
        assert!(
            hold_late > hold_early && (hold_late - hold_peak1).abs() < 0.02,
            "hold_early={hold_early}, hold_late={hold_late}, peak={hold_peak0}/{hold_peak1}"
        );
    }

    #[test]
    fn test_motion_art_hls_m3u8_parser() {
        use am_light_lib::motion_art::{
            parse_best_avc_variant_from_master, parse_mp4_url_from_variant,
        };

        let master = r#"#EXTM3U
#EXT-X-VERSION:7
#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=90000,CODECS="avc1.64001f",URI="iframe_486.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=2400000,RESOLUTION=1080x1080,CODECS="hvc1.1.6.L120.90"
hevc_1080.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=900000,RESOLUTION=486x486,CODECS="avc1.64001f"
avc_486.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=3500000,RESOLUTION=2160x2160,CODECS="avc1.64002a"
avc_2160.m3u8
"#;
        let master_url = "https://mvod.itunes.apple.com/itunes-assets/motion/master.m3u8";
        let picked = parse_best_avc_variant_from_master(master, master_url)
            .expect("must pick avc1 486x486 variant");
        assert_eq!(
            picked,
            "https://mvod.itunes.apple.com/itunes-assets/motion/avc_486.m3u8"
        );

        let variant = r#"#EXTM3U
#EXT-X-TARGETDURATION:6
#EXT-X-MAP:URI="video_486x486.mp4"
#EXTINF:5.9,
video_486x486.mp4
#EXT-X-ENDLIST
"#;
        let mp4 = parse_mp4_url_from_variant(variant, &picked).expect("must extract mp4 url");
        assert_eq!(
            mp4,
            "https://mvod.itunes.apple.com/itunes-assets/motion/video_486x486.mp4"
        );
    }

    #[test]
    fn test_remote_version_semver_compare() {
        use am_light_lib::app_ui::is_remote_version_newer;

        assert!(is_remote_version_newer("0.2.1", "v0.2.2"));
        assert!(is_remote_version_newer("0.2.1", "v0.10.0"));
        assert!(is_remote_version_newer("0.2.1", "1.0.0"));
        assert!(!is_remote_version_newer("0.2.1", "v0.2.1"));
        assert!(!is_remote_version_newer("0.2.1", "v0.2.0"));
        assert!(!is_remote_version_newer("0.2.1", "invalid-tag"));
    }

    #[test]
    fn test_narrow_window_floating_sidebar_state() {
        use am_light_lib::app_ui::{AppUi, PageRoute, UiAction};
        use am_light_lib::theme::Theme;

        let mut ui = AppUi::new(0);
        ui.last_win_w = Theme::SIDEBAR_AUTO_COLLAPSE_WIDTH - 120.0;
        assert!(!ui.narrow_sidebar_overlay_open);

        // Toggle opens floating overlay without permanently mutating desktop sidebar_collapsed
        let prev_desktop_collapsed = ui.settings.sidebar_collapsed;
        ui.execute_action(UiAction::ToggleSidebar);
        assert!(ui.narrow_sidebar_overlay_open);
        assert_eq!(ui.sidebar_open_spring.target, 1.0);
        assert_eq!(ui.settings.sidebar_collapsed, prev_desktop_collapsed);

        // Navigating closes floating overlay automatically
        ui.execute_action(UiAction::Navigate(PageRoute::Browse));
        assert!(!ui.narrow_sidebar_overlay_open);

        // Re-open then close via scrim dismiss action
        ui.execute_action(UiAction::ToggleSidebar);
        assert!(ui.narrow_sidebar_overlay_open);
        ui.execute_action(UiAction::CloseNarrowSidebarOverlay);
        assert!(!ui.narrow_sidebar_overlay_open);
        assert_eq!(ui.sidebar_open_spring.target, 0.0);
    }

    #[test]
    fn test_zero_tear_cjk_wave_shear_and_toggle() {
        use am_light_lib::app_ui::{AppUi, UiAction};
        use am_light_lib::lyrics::eval_frontier_wave_shear;

        let (l0, s0) = eval_frontier_wave_shear(-4.0, 40.0, 0.0, false);
        assert_eq!(l0, 0.0);
        assert_eq!(s0, 0.0);

        let (l_mid, s_mid) = eval_frontier_wave_shear(-4.0, 40.0, 0.45, false);
        assert!(l_mid < -0.5 && l_mid > -3.8, "mid center lift={l_mid}");
        assert!(s_mid > 0.01 && s_mid <= 0.14, "LTR wave lifts left ahead of right: shear={s_mid}");

        let (_, s_rtl) = eval_frontier_wave_shear(-4.0, 40.0, 0.45, true);
        assert!(s_rtl < -0.01 && s_rtl >= -0.14, "RTL wave mirrors shear sign: shear={s_rtl}");

        let (l1, s1) = eval_frontier_wave_shear(-4.0, 40.0, 1.0, false);
        assert!((l1 - (-4.0)).abs() < 1e-4 && s1 == 0.0, "end settles flat: l1={l1}, s1={s1}");

        let mut ui = AppUi::new(0);
        let init_wave = ui.settings.lyrics_frontier_wave;
        ui.execute_action(UiAction::ToggleLyricsFrontierWave);
        assert_eq!(ui.settings.lyrics_frontier_wave, !init_wave);
        ui.execute_action(UiAction::ToggleLyricsFrontierWave);
        assert_eq!(ui.settings.lyrics_frontier_wave, init_wave);
    }

    #[test]
    fn test_v023_mp4_crop_and_blur_downsample_toggle() {
        use am_light_lib::app_ui::{AppUi, UiAction};
        use am_light_lib::motion_art::compute_clean_display_crop;

        // H.264 486x486 padded to 496x496 macroblock coded frame -> crop strips 10px right/bottom padding!
        assert_eq!(
            compute_clean_display_crop(496, 496, Some((0, 0, 486, 486)), 486, 486),
            (0, 0, 486, 486)
        );
        // Even when MF omits aperture after CURRENTMEDIATYPECHANGED, initial display size catches 16px rounding!
        assert_eq!(
            compute_clean_display_crop(496, 496, None, 486, 486),
            (0, 0, 486, 486)
        );
        // Exact multiple of 16 passes through unchanged
        assert_eq!(
            compute_clean_display_crop(544, 544, None, 544, 544),
            (0, 0, 544, 544)
        );

        let mut ui = AppUi::new(0);
        let init_ds = ui.settings.lyrics_blur_downsample;
        ui.execute_action(UiAction::ToggleLyricsBlurDownsample);
        assert_eq!(ui.settings.lyrics_blur_downsample, !init_ds);
        ui.execute_action(UiAction::ToggleLyricsBlurDownsample);
        assert_eq!(ui.settings.lyrics_blur_downsample, init_ds);
    }

    #[test]
    fn test_v024_adaptive_spectrogram_and_hires_specs() {
        use am_light_lib::audio_viz::{
            enrich_stream_specs_with_live_and_traits, parse_enhanced_hls_variants,
        };
        use am_light_lib::visualizers::compute_adaptive_display_max_hz;

        let nyq_192 = 96_000.0f32;
        let mut bins_20k = vec![-110.0f32; 256];
        for b in 0..54 {
            bins_20k[b] = -32.0;
        }
        // Standard 20kHz music on a 192kHz DAC MUST stay at default 24 kHz!
        let c0 = compute_adaptive_display_max_hz(24_000.0, nyq_192, &bins_20k, &bins_20k, 20_000.0, 12);
        assert!((c0 - 24_000.0).abs() < 1.0, "expected 24kHz default, got {c0}");

        // Ultrasonic energy at ~32 kHz (bins 84..88 of 256 across 96 kHz) expands chart to 48 kHz!
        let mut bins_32k = bins_20k.clone();
        for b in 84..89 {
            bins_32k[b] = -58.0;
        }
        let c1 = compute_adaptive_display_max_hz(c0, nyq_192, &bins_32k, &bins_32k, 32_000.0, 14);
        assert!((c1 - 48_000.0).abs() < 1.0, "expected 48kHz expansion, got {c1}");

        // Ultrasonic energy at ~64 kHz (bins 168..173) expands chart to 96 kHz!
        let mut bins_64k = bins_32k.clone();
        for b in 168..174 {
            bins_64k[b] = -60.0;
        }
        let c2 = compute_adaptive_display_max_hz(c1, nyq_192, &bins_64k, &bins_64k, 64_000.0, 16);
        assert!((c2 - 96_000.0).abs() < 1.0, "expected 96kHz expansion, got {c2}");

        // Reset / track change (`analyzed_frames < 2`) snaps back to 24 kHz!
        let c_reset = compute_adaptive_display_max_hz(c2, nyq_192, &bins_20k, &bins_20k, 0.0, 0);
        assert!((c_reset - 24_000.0).abs() < 1.0, "expected reset to 24kHz, got {c_reset}");

        // Hi-Res 192 kHz parsed even when SAMPLE-RATE only appears in GROUP-ID!
        let gid_only_m3u8 = "\
#EXTM3U\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-alac-stereo-192000-24\",CHANNELS=\"2\"\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-alac-stereo-48000-24\",CHANNELS=\"2\"\n\
#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=5235000,BANDWIDTH=5439000,CODECS=\"alac\",AUDIO=\"audio-alac-stereo-192000-24\"\n\
hi192.m3u8\n\
#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=1690000,BANDWIDTH=1798000,CODECS=\"alac\",AUDIO=\"audio-alac-stereo-48000-24\"\n\
lo48.m3u8\n";
        let sp192 = parse_enhanced_hls_variants(gid_only_m3u8, true, 20, 10).expect("192k specs");
        assert_eq!(sp192.sample_rate_hz, 192_000);
        assert_eq!(sp192.bit_depth, Some(24));

        // Capped `_lossless.m3u8` (48kHz only) upgraded by live AMPLibraryAgent 192kHz & hi-res trait!
        let cap_m3u8 = "\
#EXTM3U\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio-alac-stereo-48000-24\",CHANNELS=\"2\",SAMPLE-RATE=48000,BIT-DEPTH=24\n\
#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=1690000,BANDWIDTH=1798000,CODECS=\"alac\",AUDIO=\"audio-alac-stereo-48000-24\"\n\
lo48.m3u8\n";
        let base48 = parse_enhanced_hls_variants(cap_m3u8, true, 20, 10);
        let upgraded = enrich_stream_specs_with_live_and_traits(
            base48.clone(),
            Some(192_000),
            Some(24),
            Some("alac"),
            true,
            true,
            20,
            10,
        )
        .expect("upgraded 192k");
        assert_eq!(upgraded.sample_rate_hz, 192_000);
        assert_eq!(upgraded.tier_label, "Hi-Res 无损");

        let trait_only =
            enrich_stream_specs_with_live_and_traits(base48, None, None, None, true, true, 20, 10)
                .expect("trait hires");
        assert!(trait_only.hires_catalog_trait);
        assert_eq!(trait_only.tier_label, "Hi-Res 无损");
    }

    #[test]
    fn test_v024_soundstage_5band_colors_and_rms_agc() {
        use am_light_lib::audio_viz::{split_stereo_5band_soundstage, LiveVisualizerSnapshot};
        use am_light_lib::visualizers::{
            soundstage_band_color, VisualizerState, SOUNDSTAGE_BAND_COLORS,
        };
        use std::f32::consts::TAU;

        // Distinct colors for all 5 frequency bands (Low -> High)
        for i in 0..5 {
            for j in (i + 1)..5 {
                assert_ne!(
                    SOUNDSTAGE_BAND_COLORS[i], SOUNDSTAGE_BAND_COLORS[j],
                    "band colors {i} and {j} must be distinct"
                );
            }
            assert_eq!(soundstage_band_color(i as u8), SOUNDSTAGE_BAND_COLORS[i]);
        }

        // Synthesize multi-band stereo signal (80 Hz bass, 300 Hz low-mid, 1 kHz mid, 3.5 kHz high-mid, 9 kHz treble)
        let sr = 48_000u32;
        let make_stereo = |amp: f32| -> Vec<[f32; 2]> {
            (0..256)
                .map(|i| {
                    let t = (i as f32) / (sr as f32);
                    let bass = (TAU * 80.0 * t).sin() * 0.30 * amp;
                    let lm = (TAU * 300.0 * t).sin() * 0.25 * amp;
                    let mid = (TAU * 1000.0 * t).sin() * 0.22 * amp;
                    let hm = (TAU * 3500.0 * t).sin() * 0.20 * amp;
                    let tr = (TAU * 9000.0 * t).sin() * 0.18 * amp;
                    [bass + lm + mid * 0.9 + hm * 0.5 + tr, bass + lm * 0.8 + mid + hm + tr * 0.4]
                })
                .collect()
        };

        let loud_lr = make_stereo(0.85);
        let band_pts = split_stereo_5band_soundstage(&loud_lr, sr);
        let mut seen = [0usize; 5];
        for &[_, _, bf] in &band_pts {
            seen[(bf.round() as usize).min(4)] += 1;
        }
        assert!(
            seen.iter().all(|&c| c > 0),
            "all 5 frequency bands must emit stereo soundstage points: {seen:?}"
        );

        // Adaptive RMS AGC: quiet (-22 dB) audio must receive substantially higher gain than loud audio!
        let mut viz_loud = VisualizerState::default();
        let mut snap_loud = LiveVisualizerSnapshot::default();
        snap_loud.active_signal = true;
        snap_loud.peak_db_l = -4.0;
        snap_loud.peak_db_r = -4.0;
        snap_loud.rms_db = -10.0;
        snap_loud.soundstage_lr = loud_lr;
        snap_loud.soundstage_band_pts = band_pts;
        for _ in 0..40 {
            viz_loud.step(&snap_loud, 1.0 / 60.0, false);
        }

        let quiet_lr = make_stereo(0.08);
        let mut viz_quiet = VisualizerState::default();
        let mut snap_quiet = LiveVisualizerSnapshot::default();
        snap_quiet.active_signal = true;
        snap_quiet.peak_db_l = -26.0;
        snap_quiet.peak_db_r = -26.0;
        snap_quiet.rms_db = -32.0;
        snap_quiet.soundstage_lr = quiet_lr.clone();
        snap_quiet.soundstage_band_pts = split_stereo_5band_soundstage(&quiet_lr, sr);
        for _ in 0..60 {
            viz_quiet.step(&snap_quiet, 1.0 / 60.0, false);
        }
        assert!(
            viz_quiet.soundstage_gain_value() > viz_loud.soundstage_gain_value() * 3.0,
            "AGC must boost quiet volume gain significantly: quiet={} vs loud={}",
            viz_quiet.soundstage_gain_value(),
            viz_loud.soundstage_gain_value()
        );
        assert!(
            viz_loud.soundstage_band_counts().iter().all(|&c| c > 0),
            "phosphor cloud must contain all 5 frequency bands: {:?}",
            viz_loud.soundstage_band_counts()
        );
    }

    #[test]
    fn test_v024_playlist_race_cover_flow_launch_and_toggles() {
        use am_light_lib::app_ui::{AppUi, MediaCardItem, PageRoute, UiAction};
        use am_light_lib::lyrics::eval_frontier_glow_profile;
        use am_light_lib::theme::RectF;

        let mut ui = AppUi::new(0);
        ui.settings.reduce_motion = false;

        // 1. Rapid playlist switching monotonically increments `detail_req_seq`!
        let s0 = ui.detail_req_seq;
        ui.navigate_to(PageRoute::ContainerDetail {
            kind: "library-playlists".to_string(),
            id: "p.AAA".to_string(),
            title_hint: "Playlist A".to_string(),
        });
        let s1 = ui.detail_req_seq;
        ui.navigate_to(PageRoute::ContainerDetail {
            kind: "library-playlists".to_string(),
            id: "p.BBB".to_string(),
            title_hint: "Playlist B".to_string(),
        });
        let s2 = ui.detail_req_seq;
        assert!(s1 > s0 && s2 > s1, "seq must increase on rapid playlist switches: {s0}->{s1}->{s2}");

        // 2. Cover Flow Easter Egg: navigating to LibAlbums enters grid, re-clicking LibAlbums toggles Cover Flow!
        ui.lib_cards.insert(
            "albums".to_string(),
            (0..5)
                .map(|i| MediaCardItem {
                    id: format!("alb.{i}"),
                    kind: "library-albums".to_string(),
                    title: format!("Album {i}"),
                    subtitle: "Artist".to_string(),
                    description: String::new(),
                    artwork_url: None,
                    badge: None,
                    track_input: None,
                })
                .collect(),
        );
        ui.navigate_to(PageRoute::LibAlbums);
        assert!(!ui.albums_cover_flow);
        ui.navigate_to(PageRoute::LibAlbums);
        assert!(ui.albums_cover_flow, "second click on LibAlbums must enter Cover Flow");
        assert!(ui.on_cover_flow_key(0x27)); // Right arrow
        assert_eq!(ui.cover_flow_target, 1);
        ui.navigate_to(PageRoute::LibAlbums);
        assert!(!ui.albums_cover_flow, "third click on LibAlbums must exit Cover Flow");

        // 3. Hero Album Cover Launch Zoom Animation
        let src_rect = RectF::new(120.0, 260.0, 180.0, 180.0);
        ui.execute_action(UiAction::OpenContainerFromCard {
            kind: "albums".to_string(),
            id: "12345".to_string(),
            title_hint: "Test Album".to_string(),
            artwork_url: None,
            motion_video_url: None,
            card_rect: src_rect,
            card_radius: 12.0,
        });
        let anim = ui.cover_launch_anim.as_ref().expect("launch anim active");
        assert_eq!(anim.target_id, "12345");
        assert_eq!(anim.from_rect.x, 120.0);
        assert!(ui.needs_continuous_animation());

        // 4. Window maximize transition + new settings toggles + readable glow ceiling
        ui.on_window_maximize_toggled(true);
        assert_eq!(ui.win_max_trans, 0.0);
        assert!(ui.win_max_going_max);

        let init_eq = ui.settings.live_eq_icon_spectrum;
        ui.execute_action(UiAction::ToggleLiveEqIconSpectrum);
        assert_eq!(ui.settings.live_eq_icon_spectrum, !init_eq);
        ui.execute_action(UiAction::ToggleLiveEqIconSpectrum);
        assert_eq!(ui.settings.live_eq_icon_spectrum, init_eq);

        let init_mem = ui.settings.aggressive_memory_release;
        ui.execute_action(UiAction::ToggleAggressiveMemoryRelease);
        assert_eq!(ui.settings.aggressive_memory_release, !init_mem);
        ui.execute_action(UiAction::ToggleAggressiveMemoryRelease);
        assert_eq!(ui.settings.aggressive_memory_release, init_mem);

        // Emphasized glow profile must stay legible (never saturated to 1.0 white blowout)
        let (body_mid, peak_mid, _) = eval_frontier_glow_profile(0.5, 0.0, 0.0, false, true, 1.0);
        assert!(body_mid <= 0.65 && peak_mid <= 0.80, "glow must stay readable: body={body_mid}, peak={peak_mid}");
    }

    #[test]
    fn test_v025_cjk_glow_legibility_end_hold_toggle_aurora_and_liquid_glass() {
        use am_light_lib::app_ui::{AppSettings, AppUi, UiAction};
        use am_light_lib::d2d_renderer::D2DRenderer;
        use am_light_lib::lyrics::{cjk_glow_density_scale, eval_frontier_glow_profile_ex};
        use am_light_lib::lyrics_view::LyricsViewState;

        // 1. CJK (Chinese Hanzi & Japanese Kanji/Kana) glow density compensation keeps dense strokes sharp
        let s_latin = cjk_glow_density_scale("Attention");
        let s_kanji = cjk_glow_density_scale("響鬱愛");
        let s_kana = cjk_glow_density_scale("ありがとう");
        assert!((s_latin - 1.0).abs() < 1e-4, "Latin scale must remain 1.0, got {s_latin}");
        assert!(
            s_kanji < 0.52 && s_kanji >= 0.42,
            "Chinese/Kanji stroke-density scale must tame inner glow blowout: {s_kanji}"
        );
        assert!(
            s_kana < 0.65 && s_kana > s_kanji,
            "Japanese Kana scale must be moderate between Kanji and Latin: {s_kana}"
        );

        // 2. End-of-word hold glow ramp toggle (`lyrics_end_hold_glow`)
        let (on_early, on_peak0, _) =
            eval_frontier_glow_profile_ex(1.0, 0.02, 0.0, true, true, 0.85, true);
        let (on_late, on_peak1, _) =
            eval_frontier_glow_profile_ex(1.0, 0.45, 0.0, true, true, 0.85, true);
        assert!(
            on_late > on_early + 0.08 && (on_late - on_peak1).abs() < 0.02,
            "end_hold_glow=true must smoothly intensify body glow toward peak: early={on_early}, late={on_late}, p={on_peak0}/{on_peak1}"
        );

        let (off_early, off_peak0, _) =
            eval_frontier_glow_profile_ex(1.0, 0.02, 0.0, true, true, 0.85, false);
        let (off_late, off_peak1, _) =
            eval_frontier_glow_profile_ex(1.0, 0.45, 0.0, true, true, 0.85, false);
        assert!(
            off_late <= off_early + 1e-4 && off_late < on_late - 0.20,
            "end_hold_glow=false must NOT surge whole-word brightness during end-hold: off={off_early}->{off_late}, on_late={on_late}, peaks={off_peak0}->{off_peak1}"
        );

        // Toggle persistence & legacy default=true
        let legacy: AppSettings = serde_json::from_str(r#"{"storefront":"jp"}"#).expect("legacy");
        assert!(legacy.lyrics_end_hold_glow, "legacy settings default lyrics_end_hold_glow=true");
        let mut ui = AppUi::new(0);
        let init_eh = ui.settings.lyrics_end_hold_glow;
        ui.execute_action(UiAction::ToggleLyricsEndHoldGlow);
        assert_eq!(ui.settings.lyrics_end_hold_glow, !init_eh);
        ui.execute_action(UiAction::ToggleLyricsEndHoldGlow);
        assert_eq!(ui.settings.lyrics_end_hold_glow, init_eh);

        // 3. Reduce-motion / reduce-transparency must NOT freeze Lyrics flowing aurora (`mesh_phase`)
        let mut lv = LyricsViewState::default();
        let p0 = lv.mesh_phase;
        lv.step(None, false, "trk:1", 10.0, true, 0.35, None, 720.0, 0.05, true, true);
        assert!(
            lv.mesh_phase > p0 + 0.01,
            "Lyrics flowing aurora phase must advance even when reduce_motion=true: {} -> {}",
            p0,
            lv.mesh_phase
        );

        // 4. Redesigned Liquid Glass vibrancy ColorMatrix preserves neutral gray & boosts chromatic saturation
        let m = D2DRenderer::build_glass_vibrancy_matrix(1.42, 1.05, 0.022);
        // Apply row-vector [R, G, B, 1, 1] * M to neutral gray (0.5, 0.5, 0.5)
        let gray_r = 0.5 * (m[0] + m[4] + m[8]) + m[16];
        let gray_g = 0.5 * (m[1] + m[5] + m[9]) + m[17];
        let gray_b = 0.5 * (m[2] + m[6] + m[10]) + m[18];
        assert!(
            (gray_r - gray_g).abs() < 1e-4 && (gray_g - gray_b).abs() < 1e-4,
            "vibrancy matrix must preserve neutral gray balance: ({gray_r}, {gray_g}, {gray_b})"
        );
        // Apply to saturated red-orange (0.7, 0.25, 0.2) -> chroma spread (R - B) must grow > 1.35x!
        let r_out = 0.7 * m[0] + 0.25 * m[4] + 0.2 * m[8] + m[16];
        let b_out = 0.7 * m[2] + 0.25 * m[6] + 0.2 * m[10] + m[18];
        assert!(
            (r_out - b_out) > (0.7 - 0.2) * 1.35,
            "vibrancy matrix must meaningfully enrich color saturation: spread={}",
            r_out - b_out
        );
    }

    #[test]
    fn test_v026_radio_limit_inspector_copy_soundstage_bpm_and_search_editing() {
        use am_light_lib::am_agent::AudioPrefs;
        use am_light_lib::am_api::{clamp_station_next_tracks_limit, STATION_NEXT_TRACKS_MAX_LIMIT};
        use am_light_lib::app_ui::{AppUi, PageRoute};
        use am_light_lib::audio_viz::{LiveVisualizerSnapshot, StreamAudioSpecs, TempoTracker};
        use am_light_lib::theme::RectF;
        use am_light_lib::visualizers::{
            format_expected_quality_lines, VisualizerState, VizHitAction, VizHitZone, VizStyle,
        };

        // 1. Radio station next-tracks API limit strictly clamped to <= 10 (fixes HTTP 400 Invalid Parameter Value)
        assert_eq!(STATION_NEXT_TRACKS_MAX_LIMIT, 10);
        assert_eq!(clamp_station_next_tracks_limit(15), 10);
        assert_eq!(clamp_station_next_tracks_limit(25), 10);
        assert_eq!(clamp_station_next_tracks_limit(10), 10);
        assert_eq!(clamp_station_next_tracks_limit(0), 1);

        // 2. Restrained 3-line Audio Inspector copy (`Playing \n 高解析度无损 \n 24-bit / 192 kHz · ALAC  @5440 kbps`)
        let hires_spec = StreamAudioSpecs {
            codec: "ALAC".to_string(),
            codec_label: "ALAC (Apple Lossless)".to_string(),
            tier_label: "Hi-Res 无损".to_string(),
            sample_rate_hz: 192_000,
            bit_depth: Some(24),
            channels: "2.0 立体声".to_string(),
            bitrate_kbps: 5440,
            peak_bitrate_kbps: 6200,
            uncompressed_pcm_kbps: Some(9216),
            compression_ratio_pct: Some(59.0),
            atmos_fallback_reason: None,
            hires_catalog_trait: true,
            available_variants: Vec::new(),
        };
        let prefs = AudioPrefs::default();
        let snap = LiveVisualizerSnapshot::default();
        let (l1, l2, l3) = format_expected_quality_lines(
            "Playing",
            Some(&hires_spec),
            None,
            &prefs,
            &snap,
        );
        assert_eq!(l1, "Playing");
        assert_eq!(l2, "高解析度无损");
        assert_eq!(l3, "24-bit / 192 kHz · ALAC  @5440 kbps");

        // 3. Soundstage defaults to blue monochrome (`soundstage_multicolor == false`) & toggles via click
        let viz_def = VisualizerState::default();
        assert!(
            !viz_def.soundstage_multicolor,
            "Soundstage must default to monochrome blue dots"
        );
        let mut ui = AppUi::new(0);
        ui.route = PageRoute::Visualizer;
        ui.viz_state.style = VizStyle::AudioInspector;
        assert!(!ui.viz_state.soundstage_multicolor);
        ui.viz_state.hit_zones.push(VizHitZone {
            rect: RectF::new(20.0, 420.0, 400.0, 220.0),
            action: VizHitAction::ToggleSoundstageColorMode,
        });
        let _ = ui.on_mouse_down(150.0, 500.0);
        assert!(ui.on_mouse_up(150.0, 500.0));
        assert!(
            ui.viz_state.soundstage_multicolor,
            "First click on Soundstage must switch to 5-band colors"
        );
        let _ = ui.on_mouse_down(150.0, 500.0);
        assert!(ui.on_mouse_up(150.0, 500.0));
        assert!(
            !ui.viz_state.soundstage_multicolor,
            "Second click on Soundstage must switch back to blue monochrome"
        );

        // 4. Upgraded SuperFlux + Hybrid Tempogram BPM accuracy on realistic pop backbeats,
        //    192 kHz Hi-Res sample rate, and quiet -28 dBFS audio with vocal vibrato!
        let synth_backbeat = |sr: u32, target_bpm: f32, gain: f32, add_vibrato: bool| -> f32 {
            let mut tracker = TempoTracker::new(sr);
            let beat_period = 60.0 / (target_bpm as f64);
            let total_frames = (11.0 * sr as f64) as usize;
            let mut pcm: Vec<[f32; 2]> = Vec::with_capacity(total_frames);
            for n in 0..total_frames {
                let t = (n as f64) / (sr as f64);
                let beat_pos = t / beat_period;
                let beat_idx = beat_pos.floor() as u64;
                let within_beat = (beat_pos - beat_pos.floor()) * beat_period;

                // Realistic Pop/Rock backbeat: Kick ONLY on beats 0 & 2, Snare ONLY on beats 1 & 3!
                let kick = if beat_idx % 2 == 0 {
                    let k_env = (-within_beat * 24.0).exp() as f32;
                    let k_freq = 56.0 + 95.0 * (-within_beat * 50.0).exp();
                    (2.0 * std::f64::consts::PI * k_freq * within_beat).sin() as f32 * k_env * 0.65
                } else {
                    0.0
                };
                let snare = if beat_idx % 2 == 1 {
                    let s_env = (-within_beat * 22.0).exp() as f32;
                    let noise = (((n as u64)
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1)
                        >> 33) as f32
                        / (u32::MAX as f32 / 2.0))
                        - 1.0;
                    let body = (2.0 * std::f64::consts::PI * 220.0 * within_beat).sin() as f32;
                    (noise * 0.62 + body * 0.38) * s_env * 0.52
                } else {
                    0.0
                };
                // 8th-note light hi-hat
                let eighth = ((beat_pos * 2.0).fract()) * (beat_period * 0.5);
                let hat = (2.0 * std::f64::consts::PI * 7200.0 * t).sin() as f32
                    * (-eighth * 70.0).exp() as f32
                    * 0.10;
                // Genuine continuous FM vocal/pad vibrato interference (440 ± 18 Hz @ 5.8 Hz)
                let pad = if add_vibrato {
                    let phase = 2.0 * std::f64::consts::PI * 440.0 * t
                        + (18.0 / 5.8) * (2.0 * std::f64::consts::PI * 5.8 * t).sin();
                    (phase.sin() as f32) * 0.18
                } else {
                    0.0
                };
                let s = ((kick + snare + hat + pad) * gain).clamp(-0.98, 0.98);
                pcm.push([s, s]);
            }
            let chunk_sz = (sr as usize / 100).max(441);
            for ch in pcm.chunks(chunk_sz) {
                tracker.observe_stereo_pcm(ch);
            }
            tracker.locked_bpm
        };

        for &(sr, target, gain, vib, desc) in &[
            (48_000u32, 92.0f32, 0.90f32, false, "48k pop backbeat 92 BPM"),
            (48_000, 118.0, 0.90, true, "48k pop backbeat + vibrato 118 BPM"),
            (48_000, 142.0, 0.90, true, "48k rock/jpop backbeat 142 BPM"),
            (48_000, 172.0, 0.90, false, "48k anime/dnb fast backbeat 172 BPM"),
            (192_000, 128.0, 0.90, true, "192k Hi-Res decimated 128 BPM"),
            (48_000, 108.0, 0.04, true, "quiet -28 dBFS AGC + vibrato 108 BPM"),
        ] {
            let est = synth_backbeat(sr, target, gain, vib);
            let err = (est - target).abs();
            assert!(
                err <= 1.8,
                "TempoTracker v0.2.6 failed on {desc}: expected {target} BPM, got {est:.2} BPM (err={err:.2})"
            );
        }

        // 5. Search box full text editing (`Ctrl+A`, `Ctrl+C`, `Ctrl+X`, `Ctrl+V`, arrows, Home/End, Delete, Shift+Select)
        let mut sui = AppUi::new(0);
        sui.search_focused = true;
        for ch in "周杰伦 Jay".chars() {
            assert!(sui.on_char_input(ch));
        }
        assert_eq!(sui.search_query, "周杰伦 Jay");
        assert_eq!(sui.search_cursor, sui.search_query.len());

        // Ctrl+A selects all, Ctrl+C copies full query
        let (ok_a, clip_a) = sui.on_search_key_down(b'A' as u16, true, false);
        assert!(ok_a && clip_a.is_none());
        assert_eq!(sui.search_selection_range(), Some((0, sui.search_query.len())));
        let (ok_c, clip_c) = sui.on_search_key_down(b'C' as u16, true, false);
        assert!(ok_c && clip_c.as_deref() == Some("周杰伦 Jay"));

        // Typing while all-selected replaces entire string cleanly
        assert!(sui.on_char_input('晴'));
        assert!(sui.on_char_input('天'));
        assert_eq!(sui.search_query, "晴天");
        assert_eq!(sui.search_selection_range(), None);

        // Shift+Left selects last CJK character "天", Ctrl+X cuts it
        let (ok_sl, _) = sui.on_search_key_down(0x25 /* VK_LEFT */, false, true);
        assert!(ok_sl);
        assert_eq!(sui.search_selected_text().as_deref(), Some("天"));
        let (ok_x, clip_x) = sui.on_search_key_down(b'X' as u16, true, false);
        assert!(ok_x && clip_x.as_deref() == Some("天"));
        assert_eq!(sui.search_query, "晴");

        // Paste inserts at cursor; Home + Delete removes first character
        assert!(sui.on_paste_text("天 稻香"));
        assert_eq!(sui.search_query, "晴天 稻香");
        let (ok_home, _) = sui.on_search_key_down(0x24 /* VK_HOME */, false, false);
        assert!(ok_home && sui.search_cursor == 0);
        let (ok_del, _) = sui.on_search_key_down(0x2E /* VK_DELETE */, false, false);
        assert!(ok_del);
        assert_eq!(sui.search_query, "天 稻香");

        // End + Ctrl+A + Backspace clears cleanly
        let (ok_end, _) = sui.on_search_key_down(0x23 /* VK_END */, false, false);
        assert!(ok_end && sui.search_cursor == sui.search_query.len());
        let _ = sui.on_search_key_down(b'A' as u16, true, false);
        assert!(sui.on_char_input('\u{08}'));
        assert_eq!(sui.search_query, "");
        assert_eq!(sui.search_cursor, 0);
    }

    #[test]
    fn test_v027_multi_backend_switching_cookies_flac_specs_and_live_apis() {
        use am_light_lib::app_ui::{
            get_active_playback_backend, AppSettings, AppUi, PageRoute, UiAction,
        };
        use am_light_lib::backends::{NetEaseQuality, PlaybackBackend, QQQuality};
        use am_light_lib::native_player::{detect_audio_magic_ext, parse_flac_streaminfo};
        use am_light_lib::netease_api;
        use am_light_lib::qq_api;

        // 1. Legacy AppSettings JSON backward compatibility: defaults cleanly to AppleMusicNative
        let legacy: AppSettings =
            serde_json::from_str(r#"{"storefront":"jp","use_builtin_service":true}"#)
                .expect("legacy settings deserialize");
        assert_eq!(legacy.playback_backend, PlaybackBackend::AppleMusicNative);
        assert_eq!(legacy.netease_quality, NetEaseQuality::ExHigh);
        assert_eq!(legacy.qq_quality, QQQuality::Mp3_320);

        // 2. Roundtrip serialization of all 5 PlaybackBackend variants + headings & motion support
        for &be in &PlaybackBackend::ALL {
            let s = serde_json::to_string(&be).unwrap();
            let back: PlaybackBackend = serde_json::from_str(&s).unwrap();
            assert_eq!(back, be);
            assert!(!be.short_label().is_empty());
            assert!(!be.display_name().is_empty());
            assert!(!be.home_heading().is_empty());
            assert!(!be.search_placeholder().is_empty());
        }
        assert!(PlaybackBackend::AppleMusicNative.supports_motion_artwork());
        assert!(PlaybackBackend::AppleMusicWeb.supports_motion_artwork());
        assert!(!PlaybackBackend::NetEase.supports_motion_artwork());
        assert!(!PlaybackBackend::QQMusic.supports_motion_artwork());

        // 3. UI backend switching updates settings, global active backend, clears stale caches cleanly
        let mut ui = AppUi::new(0);
        ui.execute_action(UiAction::SetPlaybackBackend(PlaybackBackend::NetEase));
        assert_eq!(ui.settings.playback_backend, PlaybackBackend::NetEase);
        assert_eq!(get_active_playback_backend(), PlaybackBackend::NetEase);
        ui.execute_action(UiAction::SetNetEaseQuality(NetEaseQuality::HiRes));
        assert_eq!(ui.settings.netease_quality, NetEaseQuality::HiRes);

        ui.execute_action(UiAction::SetPlaybackBackend(PlaybackBackend::QQMusic));
        assert_eq!(ui.settings.playback_backend, PlaybackBackend::QQMusic);
        assert_eq!(get_active_playback_backend(), PlaybackBackend::QQMusic);
        ui.execute_action(UiAction::SetQQQuality(QQQuality::Flac));
        assert_eq!(ui.settings.qq_quality, QQQuality::Flac);

        ui.execute_action(UiAction::SetPlaybackBackend(PlaybackBackend::AppleMusicWeb));
        assert_eq!(ui.settings.playback_backend, PlaybackBackend::AppleMusicWeb);
        assert_eq!(get_active_playback_backend(), PlaybackBackend::AppleMusicWeb);

        ui.execute_action(UiAction::SetPlaybackBackend(PlaybackBackend::ExternalSmtc));
        assert_eq!(ui.settings.playback_backend, PlaybackBackend::ExternalSmtc);

        // Restore default AppleMusicNative so subsequent tests remain isolated
        ui.execute_action(UiAction::SetPlaybackBackend(PlaybackBackend::AppleMusicNative));
        assert_eq!(get_active_playback_backend(), PlaybackBackend::AppleMusicNative);

        // 4. NetEase & QQ Cookie normalization / parsing & smart Settings paste routing
        assert_eq!(
            netease_api::normalize_netease_cookie("foo=1; MUSIC_U=abcdef123456; __csrf=xyz999; bar=2"),
            "MUSIC_U=abcdef123456; __csrf=xyz999"
        );
        let (uin, keyst) =
            qq_api::parse_qq_cookie_fields("pgv_pvid=1; uin=o012345678; qm_keyst=Q_H_L_5XYZ999; foo=bar");
        assert_eq!(uin, "12345678");
        assert_eq!(keyst, "Q_H_L_5XYZ999");

        ui.route = PageRoute::Settings;
        ui.settings.playback_backend = PlaybackBackend::NetEase;
        assert!(ui.on_paste_text("MUSIC_U=test_token_value_1234567890"));
        assert!(ui.settings.netease_cookie.contains("MUSIC_U=test_token_value_1234567890"));
        ui.execute_action(UiAction::ClearNetEaseAuth);
        assert!(ui.settings.netease_cookie.is_empty());

        ui.settings.playback_backend = PlaybackBackend::QQMusic;
        assert!(ui.on_paste_text("uin=o088887777; qm_keyst=Q_H_L_DEMO123"));
        assert_eq!(ui.settings.qq_uin, "88887777");
        ui.execute_action(UiAction::ClearQQAuth);
        assert!(ui.settings.qq_cookie.is_empty());
        ui.settings.playback_backend = PlaybackBackend::AppleMusicNative;

        // 5. Native audio container magic detection & Hi-Res 24-bit/96kHz FLAC STREAMINFO parsing
        assert_eq!(detect_audio_magic_ext(b"fLaC\x00\x00\x00\x22"), Some("flac"));
        assert_eq!(detect_audio_magic_ext(b"\x00\x00\x00\x20ftypM4A "), Some("m4a"));
        assert_eq!(detect_audio_magic_ext(b"ID3\x03\x00\x00"), Some("mp3"));

        // Construct valid 42-byte FLAC header: sr=96000 (0x17700), ch=2 (ch-1=1), bps=24 (bps-1=23)
        let mut flac_hdr = [0u8; 42];
        flac_hdr[0..4].copy_from_slice(b"fLaC");
        flac_hdr[4] = 0x00; // STREAMINFO block type 0
        let sr: u32 = 96_000;
        let ch_m1: u32 = 1; // 2 channels
        let bps_m1: u32 = 23; // 24-bit
        flac_hdr[18] = ((sr >> 12) & 0xFF) as u8;
        flac_hdr[19] = ((sr >> 4) & 0xFF) as u8;
        flac_hdr[20] = (((sr & 0x0F) << 4) | ((ch_m1 & 0x07) << 1) | ((bps_m1 >> 4) & 0x01)) as u8;
        flac_hdr[21] = ((bps_m1 & 0x0F) << 4) as u8;
        assert_eq!(parse_flac_streaminfo(&flac_hdr), Some((96_000, 2, 24)));

        // 6. Live end-to-end verification of NetEase & QQ Music catalog, QR matrix, stream URL & lyrics
        let rt = tokio::runtime::Runtime::new().expect("tokio rt");
        rt.block_on(async {
            // NetEase Home + Search + QR code matrix + Direct Stream URL + Direct YRC/LRC Lyrics
            let ne_home = netease_api::fetch_home_and_browse(None)
                .await
                .expect("NetEase home fetch");
            assert!(
                !ne_home.shelves.is_empty() && !ne_home.songs.is_empty(),
                "NetEase home shelves & songs must populate"
            );
            let qr = netease_api::start_qr_login()
                .await
                .expect("NetEase QR login key");
            assert!(qr.qr_width >= 21 && qr.qr_modules.len() == qr.qr_width * qr.qr_width);

            let ne_stream = netease_api::resolve_song_stream("ne:1973665667", NetEaseQuality::ExHigh, None)
                .await
                .expect("NetEase free track stream url");
            assert!(ne_stream.url.starts_with("http"));

            let ne_lyr = netease_api::fetch_lyrics_by_id("ne:1973665667", None).await;
            assert!(
                ne_lyr.as_ref().is_some_and(|l| !l.lines.is_empty()),
                "NetEase direct song-id lyrics must resolve"
            );

            // QQ Music Home + Radio + Radio Tracks + Direct Stream URL + Direct LRC Lyrics
            let qq_home = qq_api::fetch_home_and_browse(None)
                .await
                .expect("QQ Music home fetch");
            assert!(
                !qq_home.shelves.is_empty() && !qq_home.songs.is_empty(),
                "QQ Music home shelves & songs must populate"
            );
            let (qq_rshelves, _) = qq_api::fetch_radio_shelves(None)
                .await
                .expect("QQ Music radio shelves");
            assert!(!qq_rshelves.is_empty(), "QQ Music radio shelves must populate");

            let qq_radio_tracks = qq_api::fetch_radio_tracks("199", None)
                .await
                .expect("QQ Music radio tracks");
            assert!(!qq_radio_tracks.is_empty(), "QQ Music radio tracks must populate");

            let qq_stream = qq_api::resolve_song_stream("qq:002lzYCa14usQ5", None, QQQuality::M4a, None)
                .await
                .expect("QQ Music free track vkey url");
            assert!(qq_stream.url.starts_with("http"));

            let qq_lyr = qq_api::fetch_lyrics_by_mid("qq:002lzYCa14usQ5", None).await;
            assert!(
                qq_lyr.as_ref().is_some_and(|l| !l.lines.is_empty()),
                "QQ Music direct songmid lyrics must resolve"
            );
        });
    }

    #[test]
    fn test_v028_motion_viewport_detail_cover_backend_switch_and_track_desync() {
        use am_light_lib::app_ui::{AppUi, ContainerDetailData, PageRoute, UiAction};
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::motion_art::MotionArtManager;
        use am_light_lib::smtc::SmtcState;
        use am_light_lib::theme::RectF;

        // 1. URL-keyed viewport motion art supports >3 visible cards simultaneously & stops offscreen cards immediately
        let mgr = MotionArtManager::new();
        let screen1: Vec<String> = (1..=6)
            .map(|i| format!("https://mvod.itunes.apple.com/mock_motion_{i}.m3u8"))
            .collect();
        mgr.sync_visible_urls(&screen1);
        let active1 = mgr.active_slot_urls();
        assert_eq!(active1.len(), 6, "all 6 visible viewport covers must be active");

        // Scroll down: cards 1..=2 leave viewport, cards 3..=6 stay, cards 7..=8 enter
        let screen2: Vec<String> = (3..=8)
            .map(|i| format!("https://mvod.itunes.apple.com/mock_motion_{i}.m3u8"))
            .collect();
        mgr.sync_visible_urls(&screen2);
        let active2 = mgr.active_slot_urls();
        assert_eq!(active2.len(), 6);
        assert!(
            !active2.iter().any(|u| u.ends_with("_1.m3u8") || u.ends_with("_2.m3u8")),
            "scrolled-out covers must stop playing immediately"
        );
        assert!(
            active2.iter().any(|u| u.ends_with("_7.m3u8") && MotionArtManager::slot_key_for_url(u).starts_with("u_")),
            "newly visible covers must use stable URL-keyed slots"
        );
        mgr.sync_visible_urls(&[]);
        assert!(mgr.active_slot_urls().is_empty(), "leaving Home must stop all Home motion covers");

        // 2. Cover launch animation preserves artwork & motion URL while detail_loading == true (zero black flash!)
        let mut ui = AppUi::new(0);
        ui.execute_action(UiAction::OpenContainerFromCard {
            kind: "albums".to_string(),
            id: "alb-999".to_string(),
            title_hint: "Motion Album".to_string(),
            artwork_url: Some("https://is1-ssl.mzstatic.com/art/600x600bb.jpg".to_string()),
            motion_video_url: Some("https://mvod.itunes.apple.com/detail_hero.m3u8".to_string()),
            card_rect: RectF::new(40.0, 200.0, 200.0, 200.0),
            card_radius: 14.0,
        });
        assert!(ui.detail_loading);
        assert_eq!(ui.pending_detail_id, "alb-999");
        assert_eq!(ui.pending_detail_title, "Motion Album");
        assert_eq!(
            ui.pending_detail_artwork.as_deref(),
            Some("https://is1-ssl.mzstatic.com/art/600x600bb.jpg")
        );
        assert_eq!(
            ui.pending_detail_motion_url.as_deref(),
            Some("https://mvod.itunes.apple.com/detail_hero.m3u8")
        );
        // Even after cover_launch_anim completes while still loading, pending cover remains intact
        ui.cover_launch_anim = None;
        assert!(ui.pending_detail_artwork.is_some());

        // 3. Backend switch after paused Apple Music Native strictly isolates stale AM state
        ui.settings.playback_backend = PlaybackBackend::AppleMusicNative;
        ui.smtc = SmtcState {
            active: true,
            title: "AM Track".to_string(),
            artist: "AM Artist".to_string(),
            status: "Paused".to_string(),
            position_sec: 42.0,
            duration_sec: 210.0,
            backend_mode: "agent".to_string(),
            adam_id: Some("10001".to_string()),
            ..Default::default()
        };
        ui.switch_playback_backend(PlaybackBackend::NetEase);
        assert_eq!(ui.settings.playback_backend, PlaybackBackend::NetEase);
        assert!(ui.smtc.title.is_empty(), "switching backend must clear departed backend metadata");
        // Stale AM Native snapshot must be rejected while on NetEase
        ui.on_playback_state_changed(SmtcState {
            active: true,
            title: "Stale AM Track".to_string(),
            status: "Paused".to_string(),
            backend_mode: "agent".to_string(),
            ..Default::default()
        });
        assert!(ui.smtc.title.is_empty(), "stale AM Native update must never overwrite NetEase state");
        // Legitimate NetEase update must be accepted immediately
        ui.on_playback_state_changed(SmtcState {
            active: true,
            title: "晴天".to_string(),
            artist: "周杰伦".to_string(),
            status: "Playing".to_string(),
            position_sec: 40.0,
            duration_sec: 269.0,
            backend_mode: "netease".to_string(),
            adam_id: Some("ne:186016".to_string()),
            position_is_calibrated: true,
            ..Default::default()
        });
        assert_eq!(ui.smtc.title, "晴天");
        assert_eq!(ui.smtc.status, "Playing");

        // 4. NetEase Next-track command immediately resets playhead to 0:00 and holds buffering (zero 40s lyrics jump!)
        ui.execute_action(UiAction::NextTrack);
        assert_eq!(ui.smtc.position_sec, 0.0, "playhead must immediately reset to 0:00 on track switch");
        assert_eq!(ui.smtc.status, "Paused", "status must pause/buffer while next track audio loads");
        assert!(ui.playback_loading, "playback_loading must stay true until next track audio starts");

        // 5. Detail page data supports motion_video_url & related_albums for Artist profile pages
        let artist_det = ContainerDetailData {
            id: "6452".to_string(),
            kind: "ne-artist".to_string(),
            title: "周杰伦".to_string(),
            subtitle: "网易云音乐 · 50 首热门 · 12 张专辑".to_string(),
            description: String::new(),
            artwork_url: None,
            motion_video_url: None,
            tracks: Vec::new(),
            related_albums: Vec::new(),
        };
        assert_eq!(artist_det.kind, "ne-artist");
        assert!(!matches!(ui.route, PageRoute::Settings));
    }

    #[test]
    fn test_v028_context_menus_playlist_transfer_matcher_and_immersive_lyrics() {
        use am_light_lib::am_agent::AgentTrackInput;
        use am_light_lib::app_ui::{AppUi, ContainerDetailData, ContextMenuTarget, UiAction};
        use am_light_lib::backends::{match_track_score, normalize_match_text, PlaybackBackend};
        use am_light_lib::lyrics_view::{eval_immersive_corner_targets, LyricsHitAction};
        use am_light_lib::netease_api;
        use am_light_lib::theme::RectF;

        // 1. Cross-platform track matcher accuracy (CJK/Latin/parenthetical/remix penalty)
        assert_eq!(normalize_match_text("晴天 (Live) [伴奏]"), "晴天");
        let exact = match_track_score("七里香", "周杰伦", 299.0, "七里香", "周杰伦", 299.5);
        let remix = match_track_score("七里香", "周杰伦", 299.0, "七里香 (DJ Remix)", "DJ 小明", 240.0);
        assert!(exact >= 0.95, "exact CJK match must score high: {exact}");
        assert!(remix < 0.54, "unwanted DJ remix must be rejected: {remix}");

        // 2. Context menu (`•••`) actions on tracks, detail page, and lyrics overlay + Transfer modal
        let mut ui = AppUi::new(0);
        let sample_tr = AgentTrackInput {
            id: "ne:186016".to_string(),
            catalog_id: None,
            kind: Some("ne-song".to_string()),
            name: "晴天".to_string(),
            artist_name: "周杰伦".to_string(),
            album_name: "叶惠美".to_string(),
            duration_sec: 269.0,
            artwork_url: None,
            audio_traits: Vec::new(),
        };
        ui.execute_action(UiAction::OpenTrackContextMenu {
            track: sample_tr.clone(),
            anchor: RectF::new(600.0, 220.0, 28.0, 28.0),
        });
        assert!(matches!(
            ui.context_menu.as_ref().map(|c| &c.target),
            Some(ContextMenuTarget::Track(_))
        ));
        ui.execute_action(UiAction::ToggleContextPlaylistSubmenu);
        assert!(ui.context_menu.as_ref().is_some_and(|c| c.show_playlist_submenu));

        // PlayNextTrack pushes into play_next_queue on Apple Music Native when a track is playing
        ui.smtc.title = "Now Playing".to_string();
        ui.execute_action(UiAction::PlayNextTrack(sample_tr.clone()));
        assert_eq!(ui.play_next_queue.len(), 1);
        assert!(ui.context_menu.is_none());

        // Cross-platform Playlist Transfer Modal
        ui.detail_data = Some(ContainerDetailData {
            id: "19723756".to_string(),
            kind: "ne-playlist".to_string(),
            title: "云音乐飙升榜".to_string(),
            subtitle: "网易云".to_string(),
            description: String::new(),
            artwork_url: None,
            motion_video_url: None,
            tracks: vec![sample_tr.clone()],
            related_albums: Vec::new(),
        });
        ui.execute_action(UiAction::OpenPlaylistTransferModal);
        assert!(ui.transfer_modal.is_some());
        ui.execute_action(UiAction::SelectTransferTarget(PlaybackBackend::AppleMusicNative));
        assert_eq!(
            ui.transfer_modal.as_ref().map(|m| m.target_backend),
            Some(PlaybackBackend::AppleMusicNative)
        );
        ui.execute_action(UiAction::ClosePlaylistTransferModal);
        assert!(ui.transfer_modal.is_none());

        // 3. Lyrics Immersive Mode toggle & proximity corner reveal math
        let win = RectF::new(0.0, 0.0, 1280.0, 760.0);
        assert_eq!(
            eval_immersive_corner_targets(win, (640.0, 380.0), 0.1, false, false),
            [1.0, 1.0, 1.0, 1.0],
            "non-immersive mode must keep all 4 corners visible"
        );
        assert_eq!(
            eval_immersive_corner_targets(win, (640.0, 380.0), 0.1, true, false),
            [0.0, 0.0, 0.0, 0.0],
            "immersive mode with pointer in center must hide all 4 corners"
        );
        assert_eq!(
            eval_immersive_corner_targets(win, (30.0, 24.0), 0.2, true, false),
            [1.0, 0.0, 0.0, 0.0],
            "moving near top-left must reveal ONLY top-left corner"
        );
        assert_eq!(
            eval_immersive_corner_targets(win, (1240.0, 24.0), 0.2, true, false),
            [0.0, 1.0, 0.0, 0.0],
            "moving near top-right must reveal ONLY top-right corner"
        );
        assert_eq!(
            eval_immersive_corner_targets(win, (1180.0, 730.0), 0.2, true, false),
            [0.0, 0.0, 0.0, 1.0],
            "moving near bottom-right must reveal ONLY bottom-right capsule"
        );
        assert_eq!(
            eval_immersive_corner_targets(win, (30.0, 24.0), 3.0, true, false),
            [0.0, 0.0, 0.0, 0.0],
            "idle cursor (>2.4s) must hide all corners again"
        );

        ui.handle_lyrics_action(LyricsHitAction::ToggleImmersiveMode);
        assert!(ui.settings.lyrics_immersive_mode);
        assert!(ui.lyrics_view.immersive_mode);
        ui.handle_lyrics_action(LyricsHitAction::OpenMoreMenu(RectF::new(400.0, 460.0, 32.0, 32.0)));
        assert!(matches!(
            ui.context_menu.as_ref().map(|c| &c.target),
            Some(ContextMenuTarget::NowPlaying)
        ));

        // 4. Live NetEase artist profile + albums resolution check
        let rt = tokio::runtime::Runtime::new().expect("tokio rt");
        rt.block_on(async {
            if let Some((aid, aname)) = netease_api::resolve_artist_by_name("周杰伦", None).await {
                assert!(!aid.is_empty() && !aname.is_empty());
                if let Ok(det) = netease_api::fetch_container_detail("ne-artist", &aid, &aname, None).await {
                    assert!(!det.tracks.is_empty(), "NetEase artist top songs must load");
                    assert!(!det.related_albums.is_empty(), "NetEase artist albums must load");
                }
            }
        });
    }

    #[test]
    fn test_i18n_languages_autonyms_templates_and_metadata_safety() {
        use am_light_lib::app_ui::{AppSettings, AppUi, UiAction};
        use am_light_lib::i18n::{
            active_resolved_language, map_win32_langid, set_active_language, translate_ui_text_for,
            AppLanguage, ResolvedLang,
        };

        // 1. Win32 LANGID mapping across all 5 locales + English fallback
        assert_eq!(map_win32_langid(0x0804), ResolvedLang::ZhCn); // zh-CN
        assert_eq!(map_win32_langid(0x1004), ResolvedLang::ZhCn); // zh-SG
        assert_eq!(map_win32_langid(0x0404), ResolvedLang::ZhTw); // zh-TW
        assert_eq!(map_win32_langid(0x0C04), ResolvedLang::ZhTw); // zh-HK
        assert_eq!(map_win32_langid(0x0409), ResolvedLang::En); // en-US
        assert_eq!(map_win32_langid(0x0411), ResolvedLang::Ja); // ja-JP
        assert_eq!(map_win32_langid(0x0412), ResolvedLang::Ko); // ko-KR
        assert_eq!(map_win32_langid(0x040C), ResolvedLang::En); // fr-FR -> En fallback

        // 2. Native autonyms never get translated away (users can always switch back)
        for &lang in &[ResolvedLang::ZhTw, ResolvedLang::En, ResolvedLang::Ja, ResolvedLang::Ko] {
            for &app_lang in &AppLanguage::ALL {
                let auto_lbl = app_lang.autonym();
                assert_eq!(
                    translate_ui_text_for(auto_lbl, lang).as_ref(),
                    auto_lbl,
                    "autonym {auto_lbl} must stay invariant under {lang:?}"
                );
            }
        }

        // 3. Exact UI dictionary & parameterized templates across all 4 non-ZhCn locales
        assert_eq!(translate_ui_text_for("设置", ResolvedLang::En), "Settings");
        assert_eq!(translate_ui_text_for("设置", ResolvedLang::ZhTw), "設定");
        assert_eq!(translate_ui_text_for("设置", ResolvedLang::Ja), "設定");
        assert_eq!(translate_ui_text_for("设置", ResolvedLang::Ko), "설정");
        assert_eq!(
            translate_ui_text_for("待播清单 (12)", ResolvedLang::En),
            "Up Next (12)"
        );
        assert_eq!(
            translate_ui_text_for("前往艺人 · 周杰伦", ResolvedLang::En),
            "Go to Artist · 周杰伦"
        );
        assert_eq!(
            translate_ui_text_for("前往艺人 · 周杰伦", ResolvedLang::Ja),
            "アーティストへ · 周杰伦"
        );
        assert_eq!(
            translate_ui_text_for("当前屏幕 144 Hz · 目标刷新率：自适应 72Hz", ResolvedLang::En),
            "Display 144 Hz · Target: Adaptive 72Hz"
        );
        assert_eq!(
            translate_ui_text_for("源歌单：「深夜电台」 · 共 25 首歌曲", ResolvedLang::En),
            "Source: \"深夜电台\" · 25 tracks"
        );
        assert_eq!(
            translate_ui_text_for("界面语言已切换为：English", ResolvedLang::En),
            "Language switched to: English"
        );

        // 4. User media metadata safety: Chinese song titles, artist names, album titles stay untouched!
        for sample in ["晴天", "周杰伦", "七里香", "夜曲", "突然好想你", "Bohemian Rhapsody"] {
            for &lang in &[
                ResolvedLang::ZhCn,
                ResolvedLang::ZhTw,
                ResolvedLang::En,
                ResolvedLang::Ja,
                ResolvedLang::Ko,
            ] {
                assert_eq!(
                    translate_ui_text_for(sample, lang).as_ref(),
                    sample,
                    "media metadata '{sample}' must NEVER be altered under {lang:?}"
                );
            }
        }

        // 5. Settings serialization & UiAction::SetAppLanguage immediate live switch
        let parsed: AppSettings = serde_json::from_str(r#"{"language":"Ja"}"#).expect("serde lang");
        assert_eq!(parsed.language, AppLanguage::Ja);

        let mut ui = AppUi::new(0);
        ui.execute_action(UiAction::SetAppLanguage(AppLanguage::En));
        assert_eq!(ui.settings.language, AppLanguage::En);
        assert_eq!(active_resolved_language(), ResolvedLang::En);
        assert_eq!(am_light_lib::i18n::am_api_locale(), "en-US");

        ui.execute_action(UiAction::SetAppLanguage(AppLanguage::Ko));
        assert_eq!(active_resolved_language(), ResolvedLang::Ko);
        assert_eq!(am_light_lib::i18n::am_api_locale(), "ko-KR");

        // Reset global atomic back to Auto cleanly
        set_active_language(AppLanguage::Auto);
    }

    #[test]
    fn test_volume_persistence_across_startup_and_backend_switches() {
        use am_light_lib::am_agent;
        use am_light_lib::app_ui::{AppSettings, AppUi, UiAction};
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::native_player;
        use am_light_lib::smtc::SmtcState;
        use am_light_lib::web_am_player;

        // 1. Settings JSON roundtrip preserves custom playback_volume & defaults legacy JSON to 0.75
        let legacy: AppSettings = serde_json::from_str(r#"{"storefront":"us"}"#).expect("legacy");
        assert!((legacy.playback_volume - 0.75).abs() < 1e-6);

        let custom: AppSettings =
            serde_json::from_str(r#"{"playback_volume":0.34}"#).expect("custom vol");
        assert!((custom.playback_volume - 0.34).abs() < 1e-6);

        // 2. Setting volume updates UI, settings, and ALL playback backend engines simultaneously
        let mut ui = AppUi::new(0);
        ui.execute_action(UiAction::SetVolumeRatio(0.28));
        assert!((ui.settings.playback_volume - 0.28).abs() < 1e-6);
        assert_eq!(ui.smtc.volume, Some(0.28));
        assert!((am_agent::get_saved_volume() - 0.28).abs() < 1e-6);
        assert!((native_player::get_saved_volume() - 0.28).abs() < 1e-6);
        assert!((web_am_player::get_saved_volume() - 0.28).abs() < 1e-5);

        // 3. Switching across ALL playback backends must NEVER reset volume back to 0.75!
        for next in [
            PlaybackBackend::NetEase,
            PlaybackBackend::QQMusic,
            PlaybackBackend::AppleMusicWeb,
            PlaybackBackend::ExternalSmtc,
            PlaybackBackend::AppleMusicNative,
        ] {
            ui.switch_playback_backend(next);
            assert!(
                (ui.settings.playback_volume - 0.28).abs() < 1e-6,
                "settings.playback_volume drifted after switch to {next:?}"
            );
            assert_eq!(
                ui.smtc.volume,
                Some(0.28),
                "smtc.volume must stay 0.28 (not None/0.75) after switch to {next:?}"
            );
            assert!((am_agent::get_saved_volume() - 0.28).abs() < 1e-6);
            assert!((native_player::get_saved_volume() - 0.28).abs() < 1e-6);
            assert!((web_am_player::get_saved_volume() - 0.28).abs() < 1e-5);
        }

        // 4. Incoming playback update with `volume: None` preserves saved volume
        let st_no_vol = SmtcState {
            active: true,
            title: "Test Track".to_string(),
            status: "Playing".to_string(),
            backend_mode: "agent".to_string(),
            volume: None,
            ..Default::default()
        };
        ui.on_playback_state_changed(st_no_vol);
        assert_eq!(ui.smtc.volume, Some(0.28));
    }

    #[test]
    fn test_netease_listen_together_eapi_crypto_invite_parser_and_sync_engine() {
        use am_light_lib::app_ui::{AppUi, UiAction};
        use am_light_lib::listen_together::{
            aes128_ecb_encrypt_pkcs7, build_share_url, eapi_encrypt_params,
            evaluate_remote_sync_action, md5_hex, parse_invite_input, parse_status_json,
            ListenTogetherStatus, RemoteSyncAction,
        };
        use serde_json::json;

        // 1. RFC 1321 MD5 test vectors
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            md5_hex(b"message digest"),
            "f96b697d7cb7938d525a2f31aaf161d0"
        );

        // 2. NIST FIPS-197 AES-128 block test vector (first 16 bytes of PKCS#7 encrypted 16-byte block)
        let nist_key: [u8; 16] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d,
            0x0e, 0x0f,
        ];
        let nist_pt: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let nist_ct_expected: [u8; 16] = [
            0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4,
            0xc5, 0x5a,
        ];
        let ct = aes128_ecb_encrypt_pkcs7(&nist_key, &nist_pt);
        assert_eq!(ct.len(), 32); // 16-byte input + full 16-byte PKCS#7 padding block
        assert_eq!(&ct[..16], &nist_ct_expected);

        // NetEase EAPI params encryption produces uppercase hex multiple of 32 chars
        let eapi_hex = eapi_encrypt_params("/api/listen/together/room/create", r#"{"refer":"songplay_more"}"#);
        assert!(!eapi_hex.is_empty() && eapi_hex.len() % 32 == 0);
        assert!(eapi_hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));

        // 3. Smart invite link & room ID parser across ALL formats
        let share = build_share_url("MzA0NjY5_room", 32953014, Some("ne:186016"));
        assert_eq!(
            share,
            "https://st.music.163.com/listen-together/share/?songId=186016&roomId=MzA0NjY5_room&inviterId=32953014"
        );
        let p1 = parse_invite_input(&format!("快来和我一起听歌吧 {share} (来自网易云)")).expect("parse share");
        assert_eq!(p1.room_id, "MzA0NjY5_room");
        assert_eq!(p1.inviter_id, Some(32953014));
        assert_eq!(p1.song_id.as_deref(), Some("186016"));

        let double_enc = "orpheus%253A%252F%252Flisten%253FroomId%253DROOM9988%2526inviterId%253D778899";
        let p2 = parse_invite_input(double_enc).expect("parse double encoded nativeUrl");
        assert_eq!(p2.room_id, "ROOM9988");
        assert_eq!(p2.inviter_id, Some(778899));

        let p3 = parse_invite_input("MYROOM123:556677").expect("parse pair");
        assert_eq!(p3.room_id, "MYROOM123");
        assert_eq!(p3.inviter_id, Some(556677));

        // 4. Status JSON parser (active room + code 488 closed room)
        let status_json = json!({
            "code": 200,
            "data": {
                "inRoom": true,
                "roomInfo": {
                    "roomId": "R_777",
                    "creatorId": 1001,
                    "roomUsers": [
                        {"userId": 1001, "nickname": "Alice", "avatarUrl": "http://p1.music.126.net/a.jpg"},
                        {"userId": 2002, "nickname": "Bob", "avatarUrl": "https://p2.music.126.net/b.jpg"}
                    ]
                },
                "playCommand": {
                    "targetSongId": "186016",
                    "playStatus": "PLAY",
                    "progress": 42500,
                    "clientSeq": 9
                }
            }
        });
        let st = parse_status_json(&status_json);
        assert!(st.in_room && !st.room_closed && st.room_id == "R_777");
        assert_eq!(st.users.len(), 2);
        assert!(st.users[0].is_host && !st.users[1].is_host);
        assert!(st.users[0].avatar_url.as_deref().unwrap().starts_with("https://"));
        assert_eq!(st.remote_song_id.as_deref(), Some("186016"));
        assert_eq!(st.remote_progress_ms, 42500);

        let closed_st = parse_status_json(&json!({"code": 488, "message": "room closed"}));
        assert!(closed_st.room_closed && !closed_st.in_room);

        // 5. Anti-loop sync decision engine (including fresh partner seq & manual force resync!)
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:111111"), true, 10_000, &st, 1.0, 9, 1001),
            RemoteSyncAction::Noop,
            "must suppress self-echo within 1.8s of local user command when seq unchanged"
        );
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:111111"), true, 10_000, &st, 5.0, 9, 1001),
            RemoteSyncAction::SwitchTrack {
                song_id: "186016".to_string(),
                play: true,
                progress_ms: 42500
            }
        );
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:186016"), true, 41_500, &st, 5.0, 9, 1001),
            RemoteSyncAction::Noop,
            "1s drift must NOT cause audio stutter seek in steady state"
        );
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:186016"), true, 35_000, &st, 5.0, 9, 1001),
            RemoteSyncAction::SeekTo { progress_ms: 42500 },
            ">2.2s drift must resync playhead"
        );
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:186016"), false, 42_500, &st, 5.0, 9, 1001),
            RemoteSyncAction::SetPlayPause {
                play: true,
                progress_ms: 42500
            }
        );
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:186016"), true, 42_500, &closed_st, 5.0, 9, 1001),
            RemoteSyncAction::RoomClosed
        );
        let empty_st = ListenTogetherStatus {
            code: 200,
            in_room: false,
            ..Default::default()
        };
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:186016"), true, 0, &empty_st, 5.0, 9, 1001),
            RemoteSyncAction::RoomClosed
        );

        // 6. UI modal open/close wiring
        let mut ui = AppUi::new(0);
        ui.execute_action(UiAction::OpenListenTogetherModal);
        assert!(ui.listen_together.modal_open);
        ui.execute_action(UiAction::CloseListenTogetherModal);
        assert!(!ui.listen_together.modal_open);
    }

    #[test]
    fn test_listen_together_v2_shortlink_vip_sync_and_queue_scroll() {
        use am_light_lib::app_ui::{AppUi, UiAction};
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::listen_together::{
            evaluate_remote_sync_action, extract_first_http_url, parse_invite_input,
            parse_sync_playlist_json, ListenTogetherUser, RemoteSyncAction,
        };
        use am_light_lib::netease_api::parse_vip_info_json;
        use am_light_lib::theme::RectF;
        use serde_json::json;

        // 1. Mobile oral copy with 163cn.tv short link
        let oral_copy = "我的耳机分你一半，和我一起听歌吧～快点开看看 https://163cn.tv/blBIyEiR (@网易云音乐)";
        assert!(
            parse_invite_input(oral_copy).is_none(),
            "unresolved 163cn.tv short link must NOT be misparsed as raw roomId"
        );
        assert_eq!(
            extract_first_http_url(oral_copy).as_deref(),
            Some("https://163cn.tv/blBIyEiR")
        );

        // 2. Authoritative `sync/playlist/get` parser + wall-clock play progress compensation
        let now_ms = 1_750_000_005_000u64;
        let sync_json = json!({
            "code": 200,
            "data": {
                "serverTime": now_ms,
                "playCommand": {
                    "targetSongId": "2730282462",
                    "formerSongId": "186016",
                    "playStatus": "PLAY",
                    "commandType": "GOTO",
                    "progress": 20_000,
                    "clientSeq": 18,
                    "userId": 2002,
                    "timestamp": now_ms - 3_000
                },
                "playlist": {
                    "version": [{"userId": 2002, "version": 7}],
                    "displayList": {
                        "result": ["2730282462", "186016", "1901371647"]
                    }
                }
            }
        });
        let st = parse_sync_playlist_json("ROOM_SYNC_1", &sync_json, now_ms);
        assert!(st.in_room && !st.room_closed);
        assert_eq!(st.remote_song_id.as_deref(), Some("2730282462"));
        assert_eq!(st.remote_progress_ms, 23_000, "must compensate 3000ms elapsed while PLAY");
        assert_eq!(st.remote_client_seq, 18);
        assert_eq!(st.remote_list_version, 7);
        assert_eq!(st.remote_cmd_user_id, 2002);
        assert_eq!(st.remote_playlist_ids, vec![2730282462, 186016, 1901371647]);

        // 3. Fresh partner command (seq 18 > last_applied 17, uid 2002 != my_uid 1001) bypasses local cooldown!
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:186016"), true, 5_000, &st, 0.3, 17, 1001),
            RemoteSyncAction::SwitchTrack {
                song_id: "2730282462".to_string(),
                play: true,
                progress_ms: 23_000
            }
        );
        // Same song, fresh partner seek by 1.2s reacts immediately!
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:2730282462"), true, 21_500, &st, 0.4, 17, 1001),
            RemoteSyncAction::SeekTo { progress_ms: 23_000 }
        );
        // Manual `force_resync` aligns even a 700ms offset!
        let mut forced = st.clone();
        forced.force_resync = true;
        assert_eq!(
            evaluate_remote_sync_action(Some("ne:2730282462"), true, 22_200, &forced, 0.2, 18, 1001),
            RemoteSyncAction::SeekTo { progress_ms: 23_000 }
        );

        // 4. NetEase VIP tier & level parser
        let (svip_ok, svip_lv, svip_lbl) = parse_vip_info_json(
            &json!({"data": {"redVipLevel": 7, "redplus": {"rights": true}, "associator": {"rights": true}}}),
            11,
        );
        assert!(svip_ok && svip_lv == 7 && svip_lbl == "黑胶 SVIP Lv.7");

        let (vip_ok, vip_lv, vip_lbl) = parse_vip_info_json(
            &json!({"data": {"redVipLevel": 5, "associator": {"rights": true}}}),
            11,
        );
        assert!(vip_ok && vip_lv == 5 && vip_lbl == "黑胶 VIP Lv.5");

        let (free_ok, free_lv, free_lbl) = parse_vip_info_json(&json!({"data": {}}), 0);
        assert!(!free_ok && free_lv == 0 && free_lbl == "普通用户");

        // 5. Queue popover scroll bounds via `on_wheel`
        let mut ui = AppUi::new(0);
        ui.show_queue_popover = true;
        ui.queue_popover_rect = RectF::new(400.0, 200.0, 336.0, 368.0);
        ui.queue_max_scroll_y = 600.0;
        ui.queue_scroll_y = 0.0;
        ui.mouse_pos = (520.0, 320.0);
        ui.on_wheel(-1.0); // scroll down
        assert!(ui.queue_scroll_y > 40.0 && ui.queue_scroll_y <= 600.0);
        ui.on_wheel(10.0); // scroll back to top
        assert_eq!(ui.queue_scroll_y, 0.0);

        // 6. Listen Together modal scroll + tab switch + NetEase backend wiring
        ui.switch_playback_backend(PlaybackBackend::NetEase);
        ui.execute_action(UiAction::OpenListenTogetherModal);
        ui.listen_together.modal_rect = RectF::new(200.0, 100.0, 584.0, 528.0);
        ui.listen_together.modal_max_scroll_y = 320.0;
        ui.mouse_pos = (400.0, 300.0);
        ui.on_wheel(-1.0);
        assert!(ui.listen_together.modal_scroll_y > 40.0);
        ui.execute_action(UiAction::LtSetModalTab(1));
        assert_eq!(ui.listen_together.modal_tab, 1);
        assert_eq!(ui.listen_together.modal_scroll_y, 0.0);

        // Verify VIP user parsing struct fields work cleanly
        let non_vip_partner = ListenTogetherUser {
            user_id: 2002,
            nickname: "好友B".to_string(),
            avatar_url: None,
            is_host: false,
            vip_type: 0,
            is_vip: false,
        };
        assert!(!non_vip_partner.is_vip);
    }

    #[test]
    fn test_netease_cookie_normalization_and_disk_persistence_roundtrip() {
        use am_light_lib::app_ui::{AppSettings, AppUi, PageRoute, UiAction};
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::netease_api::{has_music_u_cookie, normalize_netease_cookie};

        // 1. DevTools `Cookie:` header prefix MUST NOT drop `MUSIC_U`!
        let from_hdr =
            normalize_netease_cookie("Cookie: MUSIC_U=deadbeef1234567890; __csrf=csrf_ok_99; NMTID=abc");
        assert_eq!(from_hdr, "MUSIC_U=deadbeef1234567890; __csrf=csrf_ok_99");
        assert!(has_music_u_cookie(&from_hdr));

        // 2. Multi-line `Set-Cookie` with expired empty `MUSIC_U=` before real `MUSIC_U` keeps the real token
        let from_set_cookie =
            "Set-Cookie: MUSIC_U=; Max-Age=0; Path=/\r\nSet-Cookie: MUSIC_U=real_music_u_token_888; Path=/\r\nSet-Cookie: __csrf=csrf_real_777; Path=/";
        assert_eq!(
            normalize_netease_cookie(from_set_cookie),
            "MUSIC_U=real_music_u_token_888; __csrf=csrf_real_777"
        );

        // 3. DevTools Application -> Cookies tab-separated row copy
        let from_tab = "MUSIC_U\tabcdef0123456789abcdef0123456789abcdef0123456789\t.music.163.com\t/";
        assert_eq!(
            normalize_netease_cookie(from_tab),
            "MUSIC_U=abcdef0123456789abcdef0123456789abcdef0123456789"
        );

        // 4. Bare >=40-char MUSIC_U token
        let bare = "0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(normalize_netease_cookie(bare), format!("MUSIC_U={bare}"));

        // 5. Non-cookie arbitrary text MUST return empty and MUST NEVER overwrite a saved cookie!
        assert_eq!(normalize_netease_cookie("hello world"), "");
        assert_eq!(
            normalize_netease_cookie("https://163cn.tv/blBIyEiR"),
            ""
        );
        let mut ui = AppUi::new(0);
        ui.route = PageRoute::Settings;
        ui.settings.playback_backend = PlaybackBackend::NetEase;
        assert!(ui.apply_pasted_netease_cookie("Cookie: MUSIC_U=valid_cookie_9876543210; __csrf=aa11"));
        assert_eq!(
            ui.settings.netease_cookie,
            "MUSIC_U=valid_cookie_9876543210; __csrf=aa11"
        );
        assert!(ui.netease_account.logged_in);
        assert!(!ui.apply_pasted_netease_cookie("random clipboard text without cookie"));
        assert_eq!(
            ui.settings.netease_cookie,
            "MUSIC_U=valid_cookie_9876543210; __csrf=aa11",
            "invalid paste must never clobber an already saved NetEase cookie"
        );

        // 6. Real disk round-trip via isolated temp directory (simulating full client restart!)
        let tmp_dir = std::env::temp_dir().join(format!(
            "am_light_cookie_persist_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let primary_path = tmp_dir.join("roaming_settings_v2.json");
        let backup_path = tmp_dir.join("local_settings_v2.json");
        let paths = vec![primary_path.clone(), backup_path.clone()];

        let mut saved = AppSettings::default();
        saved.playback_backend = PlaybackBackend::NetEase;
        saved.netease_cookie = "MUSIC_U=persisted_token_xyz123456789; __csrf=c999".to_string();
        saved.netease_uid = 32953014;
        saved.netease_nickname = "云村测试".to_string();
        saved.netease_vip_label = "黑胶 VIP Lv.6".to_string();
        saved.netease_vip_level = 6;
        saved.netease_is_vip = true;
        saved.save_to_paths(&paths);

        let reloaded = AppSettings::load_from_paths(&paths).expect("reload settings from disk");
        assert_eq!(reloaded.playback_backend, PlaybackBackend::NetEase);
        assert_eq!(reloaded.netease_cookie, saved.netease_cookie);
        assert_eq!(reloaded.netease_uid, 32953014);
        assert_eq!(reloaded.netease_nickname, "云村测试");
        assert_eq!(reloaded.netease_vip_label, "黑胶 VIP Lv.6");
        assert_eq!(reloaded.netease_vip_level, 6);
        assert!(reloaded.netease_is_vip);

        // 7. Backup recovery: if primary file loses `netease_cookie`, backup mirror restores it!
        let mut damaged = AppSettings::default();
        damaged.playback_backend = PlaybackBackend::NetEase;
        damaged.save_to_paths(&[primary_path.clone()]);
        let recovered = AppSettings::load_from_paths(&paths).expect("recover from backup mirror");
        assert_eq!(recovered.netease_cookie, saved.netease_cookie);
        assert_eq!(recovered.netease_uid, 32953014);
        assert_eq!(recovered.netease_vip_label, "黑胶 VIP Lv.6");

        // 8. ClearNetEaseAuth inside unit-test AppUi(0) clears in-memory without touching real user files
        ui.execute_action(UiAction::ClearNetEaseAuth);
        assert!(ui.settings.netease_cookie.is_empty());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    #[ignore]
    fn generate_readme_screenshots() {
        use am_light_lib::am_api;
        use am_light_lib::app_ui::{AppUi, PageRoute, UiAction};
        use am_light_lib::audio_viz::{
            self, AudioOutputDevice, ForensicAccumulator, StreamAudioSpecs,
        };
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::d2d_renderer::D2DRenderer;
        use am_light_lib::drm_fix::DrmDiagReport;
        use am_light_lib::i18n::{set_active_language, AppLanguage};
        use am_light_lib::lyrics::parse_ttml_lyrics;
        use am_light_lib::smtc::SmtcState;
        use am_light_lib::visualizers::VizStyle;
        use std::path::PathBuf;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        unsafe extern "system" fn shot_wnd_proc(
            h: HWND,
            m: u32,
            w: WPARAM,
            l: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }

        let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("docs")
            .join("screenshots");
        std::fs::create_dir_all(&out_dir).expect("create docs/screenshots");

        // Create an invisible offscreen Win32 HWND sized for crisp 1280x760 @ 1.5x (1920x1140)
        let w_px: u32 = 1920;
        let h_px: u32 = 1140;
        let dpi_scale: f32 = 1.5;

        let hwnd = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = w!("AMLightScreenshotHost");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(shot_wnd_proc),
                hInstance: hinst,
                lpszClassName: cls,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                cls,
                PCWSTR(w!("AM Light Capture").as_ptr()),
                WS_POPUP,
                0,
                0,
                w_px as i32,
                h_px as i32,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("CreateWindowExW")
        };

        let mut renderer = D2DRenderer::new(hwnd, w_px, h_px, dpi_scale).expect("D2DRenderer::new");
        let rt = tokio::runtime::Runtime::new().expect("tokio rt");

        // Fetch real public Apple Music editorial shelves (zero user token, 100% public catalog!)
        set_active_language(AppLanguage::ZhCn);
        let shelves = rt
            .block_on(am_api::fetch_home_editorial_shelves("cn", None))
            .unwrap_or_default();
        assert!(!shelves.is_empty(), "public Apple Music shelves must load");

        // Fetch & decode artwork for top visible cards so Home is richly populated with real covers
        let mut art_urls: Vec<(String, bool)> = Vec::new();
        for (si, shelf) in shelves.iter().take(4).enumerate() {
            for item in shelf.items.iter().take(8) {
                if !item.artwork_url.is_empty() {
                    art_urls.push((item.artwork_url.clone(), si == 0));
                }
            }
        }
        let fetched_arts: Vec<(String, bool, Vec<u8>)> = rt.block_on(async {
            let mut futs = Vec::new();
            for (u, hero) in art_urls {
                futs.push(async move {
                    let bytes = am_api::fetch_artwork_bytes(&u, hero).await.ok()?;
                    Some((u, hero, bytes))
                });
            }
            let mut out = Vec::new();
            for f in futs {
                if let Some(v) = f.await {
                    out.push(v);
                }
            }
            out
        });
        for (u, hero, bytes) in &fetched_arts {
            let _ = renderer.insert_artwork_bytes(u.clone(), bytes, *hero);
        }

        // Pick a richly illustrated public track from the live shelves for MiniPlayer & Lyrics
        let hero_item = shelves
            .iter()
            .flat_map(|s| s.items.iter())
            .find(|it| {
                !it.artwork_url.is_empty()
                    && (renderer.has_artwork(&it.artwork_url, false)
                        || renderer.has_artwork(&it.artwork_url, true))
            })
            .cloned();
        let hero_art_url = hero_item
            .as_ref()
            .map(|it| it.artwork_url.clone())
            .filter(|s| !s.is_empty());
        if let Some(ref u) = hero_art_url {
            if let Some((_, _, bytes)) = fetched_arts.iter().find(|(k, _, _)| k == u) {
                let _ = renderer.insert_artwork_bytes(u.clone(), bytes, true);
            }
        }

        // Build clean privacy-free AppUi (zero personal Apple ID / cookies / tokens / device names)
        let mut ui = AppUi::new(0);
        ui.settings.language = AppLanguage::ZhCn;
        ui.settings.storefront = "cn".to_string();
        ui.settings.user_token.clear();
        ui.settings.playback_backend = PlaybackBackend::AppleMusicNative;
        ui.settings.liquid_glass_enabled = false;
        ui.settings.window_corner_radius = 14.0;
        ui.settings.lyrics_immersive_mode = false;
        ui.settings.lyrics_auto_emphasis = true;
        ui.settings.lyrics_frontier_wave = true;
        ui.settings.lyrics_end_hold_glow = true;
        ui.official_account = Default::default();
        ui.drm_diag = DrmDiagReport {
            checked: true,
            healthy: true,
            adi_dir_exists: true,
            adi_file_count: 1,
            unwritable_files: Vec::new(),
            sc_info_unwritable: false,
            hklm_redirections: Vec::new(),
            summary: "DRM 授权状态正常（1 个 ADI 身份文件均可正常读写）".to_string(),
            detail: "若原版 Apple Music 仍遇到点歌秒切、0:00 无声，可点击「一键修复」重置设备身份（自动备份）"
                .to_string(),
        };
        ui.devices = vec![AudioOutputDevice {
            id: "studio-dac".to_string(),
            name: "Studio DAC USB Audio".to_string(),
            is_default: true,
            sample_rate: 96_000,
            bit_depth: 24,
            channels: 2,
            format_tag: "PCM".to_string(),
            spatial_audio_active: false,
        }];
        ui.current_monitor_hz = 144;
        ui.home_shelves = shelves;

        let demo_specs = StreamAudioSpecs {
            codec: "alac".to_string(),
            codec_label: "ALAC".to_string(),
            tier_label: "高解析度无损".to_string(),
            sample_rate_hz: 96_000,
            bit_depth: Some(24),
            channels: "2ch".to_string(),
            bitrate_kbps: 3210,
            peak_bitrate_kbps: 3680,
            uncompressed_pcm_kbps: Some(4608),
            compression_ratio_pct: Some(69.7),
            atmos_fallback_reason: None,
            hires_catalog_trait: true,
            available_variants: Vec::new(),
        };

        let demo_track_title = hero_item
            .as_ref()
            .map(|x| x.title.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Midnight Resonance".to_string());
        let demo_track_artist = hero_item
            .as_ref()
            .map(|x| x.subtitle.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Apple Music".to_string());

        let base_smtc = SmtcState {
            active: true,
            app_id: "AMPLibraryAgent.exe".to_string(),
            title: demo_track_title.clone(),
            artist: demo_track_artist.clone(),
            album: "Spatial & Hi-Res Showcase".to_string(),
            status: "Playing".to_string(),
            position_sec: 74.0,
            duration_sec: 238.0,
            thumbnail_base64: None,
            artwork_url: hero_art_url.clone(),
            backend_mode: "agent".to_string(),
            audio_quality: Some("高解析度无损 · 24-bit / 96kHz".to_string()),
            volume: Some(0.72),
            agent_pid: None,
            adam_id: Some("1000001".to_string()),
            stream_specs: Some(demo_specs.clone()),
            sampled_at: Some(std::time::Instant::now()),
            timeline_updated_univ: 0,
            position_is_calibrated: true,
        };
        ui.on_playback_state_changed(base_smtc.clone());

        // ====================================================================
        // SHOT 1: Home Editorial Showcase (Simplified Chinese + Liquid Glass)
        // ====================================================================
        set_active_language(AppLanguage::ZhCn);
        ui.execute_action(UiAction::Navigate(PageRoute::Home));
        ui.show_lyrics_overlay = false;
        ui.scroll_y = 0.0;
        ui.scroll_target_y = 0.0;
        for _ in 0..24 {
            ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        }
        renderer.pending_capture_path = Some(out_dir.join("home.png"));
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        assert!(out_dir.join("home.png").exists(), "home.png must be written");

        // ====================================================================
        // SHOT 2: Full-Screen LyricsBlossom Aurora + Glowing Synced Word Lyrics
        // ====================================================================
        // Pick a rich colorful cover (prefer shelf 0 item #1 or #0) so aurora mesh blooms vibrantly!
        let lyrics_art_url = ui
            .home_shelves
            .first()
            .and_then(|s| s.items.get(1).or_else(|| s.items.first()))
            .map(|it| it.artwork_url.clone())
            .filter(|u| !u.is_empty())
            .or_else(|| hero_art_url.clone());
        if let Some(ref u) = lyrics_art_url {
            if let Some((_, _, bytes)) = fetched_arts.iter().find(|(k, _, _)| k == u) {
                let _ = renderer.insert_artwork_bytes(u.clone(), bytes, true);
            }
        }
        let sample_ttml = concat!(
            r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">"#,
            r#"<head><metadata><iTunesMetadata><translations><translation xml:lang="zh-Hans-CN">"#,
            r#"<text for="L1">霓虹折射在午夜微凉的雨幕里</text>"#,
            r#"<text for="L2">每一道声波都在玻璃上缓缓绽放</text>"#,
            r#"<text for="L3">听见星海在无损高频间轻声回响</text>"#,
            r#"<text for="L4">让光影与旋律在此刻停留成永恒</text>"#,
            r#"<text for="L5">穿越云端与城市沉默的霓虹灯火</text>"#,
            r#"<text for="L6">闭上双眼感受律动在指尖轻轻起伏</text>"#,
            r#"</translation></translations></iTunesMetadata></metadata></head><body><div>"#,
            r#"<p begin="00:08.000" end="00:13.200" itunes:key="L1"><span begin="00:08.000" end="00:08.900">Neon</span> <span begin="00:08.900" end="00:09.800">lights</span> <span begin="00:09.800" end="00:10.700">dissolve</span> <span begin="00:10.700" end="00:11.600">across</span> <span begin="00:11.600" end="00:13.200">midnight rain</span></p>"#,
            r#"<p begin="00:13.600" end="00:19.200" itunes:key="L2"><span begin="00:13.600" end="00:14.400">Every</span> <span begin="00:14.400" end="00:15.300">wave</span> <span begin="00:15.300" end="00:16.200">unfolds</span> <span begin="00:16.200" end="00:17.200">in</span> <span begin="00:17.200" end="00:19.200" itunes:emphasis="true">liquid glass</span></p>"#,
            r#"<p begin="00:19.600" end="00:26.200" itunes:key="L3"><span begin="00:19.600" end="00:20.400">Hear</span> <span begin="00:20.400" end="00:21.100">the</span> <span begin="00:21.100" end="00:22.400" itunes:emphasis="true">starlight</span> <span begin="00:22.400" end="00:23.800" itunes:emphasis="true">shimmering</span> <span begin="00:23.800" end="00:25.000">through</span> <span begin="00:25.000" end="00:26.200">the air</span></p>"#,
            r#"<p begin="00:26.600" end="00:32.400" itunes:key="L4"><span begin="00:26.600" end="00:27.500">Hold</span> <span begin="00:27.500" end="00:28.400">this</span> <span begin="00:28.400" end="00:29.800" itunes:emphasis="true">melody</span> <span begin="00:29.800" end="00:31.000">inside</span> <span begin="00:31.000" end="00:32.400">the glow</span></p>"#,
            r#"<p begin="00:32.800" end="00:38.500" itunes:key="L5"><span begin="00:32.800" end="00:34.100">Drifting</span> <span begin="00:34.100" end="00:35.500">softly</span> <span begin="00:35.500" end="00:36.900">past</span> <span begin="00:36.900" end="00:38.500">the quiet sky</span></p>"#,
            r#"<p begin="00:38.900" end="00:44.500" itunes:key="L6"><span begin="00:38.900" end="00:40.200">Close</span> <span begin="00:40.200" end="00:41.500">your</span> <span begin="00:41.500" end="00:43.000">eyes</span> <span begin="00:43.000" end="00:44.500">and float</span></p>"#,
            r#"</div></body></tt>"#,
        );
        let parsed_lyr = parse_ttml_lyrics(sample_ttml, "Apple Music TTML").expect("sample ttml");
        let mut lyr_smtc = base_smtc.clone();
        lyr_smtc.title = "Midnight Resonance".to_string();
        lyr_smtc.artist = "Aurora Synth Ensemble".to_string();
        lyr_smtc.album = "Liquid Glass Sessions (Hi-Res Master)".to_string();
        lyr_smtc.artwork_url = lyrics_art_url.clone();
        lyr_smtc.position_sec = 22.40;
        lyr_smtc.duration_sec = 238.0;
        ui.on_playback_state_changed(lyr_smtc);
        ui.current_lyrics_key = "demo:lyrics-showcase".to_string();
        ui.current_lyrics = Some(parsed_lyr);
        ui.lyrics_loading = false;
        ui.lyrics_view.show_translation = true;
        ui.lyrics_view.music_free_blur = false;
        ui.show_lyrics_overlay = true;
        for f in 0..60 {
            ui.smtc.position_sec = 22.35 + (f as f64) * 0.015;
            ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        }
        renderer.pending_capture_path = Some(out_dir.join("lyrics.png"));
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        assert!(out_dir.join("lyrics.png").exists(), "lyrics.png must be written");

        // ====================================================================
        // SHOT 3: WASAPI Audio Visualizer (Audio Inspector + Spectrogram + Soundstage)
        // ====================================================================
        ui.show_lyrics_overlay = false;
        ui.execute_action(UiAction::Navigate(PageRoute::Visualizer));
        ui.viz_state.style = VizStyle::AudioInspector;
        ui.viz_state.soundstage_multicolor = true;
        let mut viz_smtc = base_smtc.clone();
        viz_smtc.title = "Midnight Resonance".to_string();
        viz_smtc.artist = "Aurora Synth Ensemble".to_string();
        viz_smtc.artwork_url = lyrics_art_url.clone();
        viz_smtc.position_sec = 92.4;
        ui.on_playback_state_changed(viz_smtc);
        // Keep the host WASAPI loopback thread parked so real host DAC sample-rate/silence never races our synthetic capture signal!
        audio_viz::set_viz_subscribed(false);

        // Synthesize 460 frames of genuine zero-mean broadband 96 kHz / 24-bit studio stereo audio
        // (smooth 1/f pink musical bed across 20 Hz..38 kHz + stereo chordal harmonics -> 0% MDCT holes,
        // full edge-to-edge waterfall spectrogram, lush 5-band stereo Soundstage fan, & lively oscilloscope!)
        let sr = 96_000u32;
        let mut accum = ForensicAccumulator::new(sr);
        let block_len = 1600usize;
        let mut phase_acc = 0.0f64;
        let pi2 = std::f64::consts::TAU;
        let mut pcm_block = vec![[0.0f32; 2]; block_len];
        let mut rng_l: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut rng_r: u64 = 0xD1B5_4A32_D192_ED03;
        let mut pink_l = [0.0f32; 5];
        let mut pink_r = [0.0f32; 5];
        let mut lp_l = 0.0f32;
        let mut lp_r = 0.0f32;

        for frame_i in 0..460 {
            let fi = frame_i as f32;
            let phrase_env = 0.58 + 0.42 * (fi * 0.045).sin().abs();
            let beat_kick = (fi * 0.24).sin().max(0.0).powf(2.6);
            let air_swell = 0.55 + 0.45 * (fi * 0.065 + 0.8).cos().abs();
            let chord_idx = (frame_i / 90) % 4;
            let (f_root, f_third, f_fifth, f_lead) = match chord_idx {
                0 => (130.81, 329.63, 392.00, 1046.50), // Cmaj7
                1 => (110.00, 261.63, 329.63, 880.00),  // Am7
                2 => (87.31, 220.00, 261.63, 1318.51),  // Fmaj7
                _ => (98.00, 246.94, 293.66, 1174.66),  // G6
            };
            for s in 0..block_len {
                let t = phase_acc / (sr as f64);
                // True zero-mean white noise (-1.0 .. +1.0)
                rng_l = rng_l.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                rng_r = rng_r.wrapping_mul(6364136223846793005).wrapping_add(2876543210987654321);
                let wl = (((rng_l >> 32) as f32) / (u32::MAX as f32)) * 2.0 - 1.0;
                let wr = (((rng_r >> 32) as f32) / (u32::MAX as f32)) * 2.0 - 1.0;

                // Paul Kellet pink filter (zero-mean continuous musical spectral bed)
                pink_l[0] = 0.996 * pink_l[0] + wl * 0.028;
                pink_l[1] = 0.982 * pink_l[1] + wl * 0.035;
                pink_l[2] = 0.945 * pink_l[2] + wl * 0.052;
                pink_l[3] = 0.840 * pink_l[3] + wl * 0.092;
                pink_l[4] = 0.500 * pink_l[4] + wl * 0.135;
                let pl = pink_l[0] + pink_l[1] + pink_l[2] + pink_l[3] + pink_l[4] + wl * 0.075 * air_swell;

                pink_r[0] = 0.996 * pink_r[0] + wr * 0.028;
                pink_r[1] = 0.982 * pink_r[1] + wr * 0.035;
                pink_r[2] = 0.945 * pink_r[2] + wr * 0.052;
                pink_r[3] = 0.840 * pink_r[3] + wr * 0.092;
                pink_r[4] = 0.500 * pink_r[4] + wr * 0.135;
                let pr = pink_r[0] + pink_r[1] + pink_r[2] + pink_r[3] + pink_r[4] + wr * 0.075 * air_swell;

                // Rich mid + wide decorrelated stereo sides + warm musical harmonic stack
                let mid_n = 0.5 * (pl + pr);
                let side_n = 0.5 * (pl - pr) * 1.65;
                let bass = (pi2 * (f_root * 0.5) * t).sin() as f32 * (0.16 + 0.18 * beat_kick)
                    + (pi2 * f_root * t).sin() as f32 * 0.12;
                let pad_l = (pi2 * f_third * t).sin() as f32 * 0.10
                    + (pi2 * f_fifth * 2.0 * t + 0.35).sin() as f32 * 0.07
                    + (pi2 * f_lead * t).sin() as f32 * 0.055
                    + (pi2 * 4400.0 * t + 0.2).sin() as f32 * 0.032 * air_swell
                    + (pi2 * 12400.0 * t).sin() as f32 * 0.022 * air_swell;
                let pad_r = (pi2 * f_fifth * t + 0.9).sin() as f32 * 0.10
                    + (pi2 * f_third * 2.0 * t + 1.4).sin() as f32 * 0.07
                    + (pi2 * f_lead * 1.5 * t + 0.6).sin() as f32 * 0.055
                    + (pi2 * 6200.0 * t + 1.1).sin() as f32 * 0.032 * air_swell
                    + (pi2 * 16800.0 * t + 0.5).sin() as f32 * 0.020 * air_swell;

                let raw_l = (mid_n + side_n) * 0.34 * phrase_env + bass + pad_l * phrase_env;
                let raw_r = (mid_n - side_n) * 0.34 * phrase_env + bass + pad_r * phrase_env;
                // Gentle ultrasonic air roll-off around ~38 kHz
                lp_l += (raw_l - lp_l) * 0.72;
                lp_r += (raw_r - lp_r) * 0.72;
                pcm_block[s] = [lp_l.clamp(-0.92, 0.92), lp_r.clamp(-0.92, 0.92)];
                phase_acc += 1.0;
            }
            audio_viz::feed_synthetic_stereo_for_capture(
                "Studio DAC USB Audio",
                sr,
                24,
                &pcm_block,
                &mut accum,
            );
            // Advance 0.045s per frame (~4.14 waterfall cols/frame) so 420px spectrogram fills 100% edge-to-edge!
            let step_dt = if frame_i < 420 { 0.045 } else { 1.0 / 60.0 };
            ui.step_and_render_ex(&mut renderer, step_dt, true);
            audio_viz::set_viz_subscribed(false);
        }
        renderer.pending_capture_path = Some(out_dir.join("visualizer.png"));
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        assert!(
            out_dir.join("visualizer.png").exists(),
            "visualizer.png must be written"
        );

        // ====================================================================
        // SHOT 4: Multi-Backend & Multi-Language Settings (English UI Showcase!)
        // ====================================================================
        ui.execute_action(UiAction::SetAppLanguage(AppLanguage::En));
        ui.execute_action(UiAction::Navigate(PageRoute::Settings));
        ui.show_lyrics_overlay = false;
        ui.scroll_y = 0.0;
        ui.scroll_target_y = 0.0;
        ui.banner_text = None;
        for _ in 0..30 {
            ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        }
        renderer.pending_capture_path = Some(out_dir.join("settings-en.png"));
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        assert!(
            out_dir.join("settings-en.png").exists(),
            "settings-en.png must be written"
        );

        set_active_language(AppLanguage::Auto);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn test_v0211_ios26_liquid_glass_inter_font_lyrics_ui_and_page_cache() {
        use am_light_lib::am_agent::AgentTrackInput;
        use am_light_lib::app_ui::{AppSettings, AppUi, ContextMenuTarget, PageRoute, UiAction};
        use am_light_lib::d2d_renderer::D2DRenderer;
        use am_light_lib::lyrics::parse_ttml_lyrics;
        use am_light_lib::lyrics_view::{LyricsHitAction, LyricsViewState};
        use am_light_lib::theme::RectF;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        unsafe extern "system" fn unit_wnd_proc(
            h: HWND,
            m: u32,
            w: WPARAM,
            l: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }

        // 1. iOS 26 Liquid Glass default ON + one-time v2 migration flag + page_cache_enabled default ON & toggle
        let def_s = AppSettings::default();
        assert!(def_s.liquid_glass_enabled, "iOS 26 Liquid Glass must default ON");
        assert!(def_s.page_cache_enabled, "page_cache_enabled must default ON");
        assert!(def_s.lyrics_left_enlarged, "lyrics_left_enlarged must default ON");
        assert!(!def_s.lyrics_center_axis, "lyrics_center_axis must default OFF");

        let uniq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let tmp_dir = std::env::temp_dir().join(format!("am_light_v0211_migrate_{uniq}"));
        let _ = std::fs::remove_dir_all(&tmp_dir);
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let old_file = tmp_dir.join("legacy_off.json");
        std::fs::write(
            &old_file,
            r#"{"storefront":"cn","liquid_glass_enabled":false}"#,
        )
        .expect("write legacy off json");
        let upgraded = AppSettings::load_from_paths(&[old_file.clone()]).expect("load legacy");
        assert!(
            upgraded.liquid_glass_enabled && upgraded.liquid_glass_v2_migrated,
            "unmigrated legacy settings with liquid_glass_enabled=false must auto-migrate ON once"
        );

        let user_opted_out = tmp_dir.join("user_opted_out.json");
        std::fs::write(
            &user_opted_out,
            r#"{"storefront":"cn","liquid_glass_enabled":false,"liquid_glass_v2_migrated":true}"#,
        )
        .expect("write opted-out json");
        let respected = AppSettings::load_from_paths(&[user_opted_out]).expect("load opted-out");
        assert!(
            !respected.liquid_glass_enabled && respected.liquid_glass_v2_migrated,
            "once liquid_glass_v2_migrated=true, user toggling Liquid Glass OFF must be respected permanently"
        );
        let _ = std::fs::remove_dir_all(&tmp_dir);

        // SDF optical lens displacement map: flat neutral interior (128,128) + curved refractive bezel!
        let disp = D2DRenderer::generate_liquid_glass_sdf_displacement_map(128, 48, 320.0, 64.0, 32.0);
        assert_eq!(disp.len(), 128 * 48 * 4);
        let center_idx = ((24 * 128) + 64) * 4;
        assert_eq!(
            (disp[center_idx + 2], disp[center_idx + 1]),
            (128, 128),
            "interior optical core must stay neutral (128,128) with zero distortion"
        );
        let edge_idx = ((24 * 128) + 2) * 4; // left bezel edge -> bends along horizontal normal
        assert!(
            (disp[edge_idx + 2] as i32 - 128).abs() >= 6,
            "outer glass bezel must encode curved horizontal refraction normal: R={}",
            disp[edge_idx + 2]
        );

        // 2. Bundled OFL Inter font loading & zero-slash tabular digits (NEVER Consolas!)
        assert!(
            D2DRenderer::verify_inter_font_bundle(),
            "bundled InterVariable.ttf must have valid TTF magic header and payload"
        );
        let d2d_src = std::fs::read_to_string("src/d2d_renderer.rs").expect("read d2d_renderer.rs");
        let has_consolas_code = d2d_src.lines().any(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with("//") && line.contains("\"Consolas\"")
        });
        assert!(
            !has_consolas_code,
            "Consolas must be completely eliminated in favor of Inter + tnum/zero=0"
        );
        assert!(
            d2d_src.contains("DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES")
                && d2d_src.contains("DWRITE_FONT_FEATURE_TAG_SLASHED_ZERO"),
            "DirectWrite mono typography must explicitly configure tnum=1 and zero=0"
        );

        // Spin up a lightweight offscreen D2D renderer to verify live DirectWrite Inter loading AND top-bar hits!
        let hwnd = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = w!("AMLightV0211UnitHost");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(unit_wnd_proc),
                hInstance: hinst,
                lpszClassName: cls,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                cls,
                PCWSTR(w!("AM Light Unit").as_ptr()),
                WS_POPUP,
                0,
                0,
                1280,
                760,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("CreateWindowExW")
        };
        let mut renderer = D2DRenderer::new(hwnd, 1280, 760, 1.0).expect("D2DRenderer::new");
        assert!(
            renderer.has_bundled_inter_font(),
            "DirectWrite in-memory font loader must load bundled Inter Variable font"
        );
        assert!(
            renderer.active_ui_font_family().contains("Inter"),
            "active UI font must be Inter, got {}",
            renderer.active_ui_font_family()
        );
        assert!(
            renderer.mono_font_family().contains("Inter")
                && !renderer.mono_font_family().contains("Consolas"),
            "mono font must be Inter (never Consolas), got {}",
            renderer.mono_font_family()
        );
        assert!(
            renderer.verify_inter_formats_all_weights(),
            "all UI weights (400/500/600/700) for regular & mono must resolve to bundled Inter"
        );

        // 3. Main window top-right Refresh button completely removed + Settings forces glass_off + page_cache toggle + F5 refresh
        let mut ui = AppUi::new(0);
        assert!(ui.settings.page_cache_enabled);
        ui.execute_action(UiAction::TogglePageCache);
        assert!(!ui.settings.page_cache_enabled);
        ui.execute_action(UiAction::TogglePageCache);
        assert!(ui.settings.page_cache_enabled);
        ui.execute_action(UiAction::ClearPageCache);
        assert!(ui.on_key_down(0x74 /* VK_F5 */));

        // Settings page must disable Liquid Glass completely while Home & Lyrics overlay retain it
        assert!(!ui.glass_off(), "Home page must have Liquid Glass ON by default");
        ui.route = PageRoute::Settings;
        ui.show_lyrics_overlay = false;
        assert!(ui.glass_off(), "Settings page must force Liquid Glass OFF for pure iPadOS matte perf");
        ui.show_lyrics_overlay = true;
        assert!(!ui.glass_off(), "Lyrics overlay opened from Settings must keep Liquid Glass ON");
        ui.show_lyrics_overlay = false;

        let app_ui_src = std::fs::read_to_string("src/app_ui.rs").expect("read src/app_ui.rs");
        let sp_start = app_ui_src
            .find("fn draw_settings_page(")
            .expect("draw_settings_page");
        let sp_rest = &app_ui_src[sp_start + 20..];
        let sp_end = sp_rest
            .find("\n    fn ")
            .map(|o| sp_start + 20 + o)
            .unwrap_or(app_ui_src.len());
        let settings_fn_src = &app_ui_src[sp_start..sp_end];
        assert!(
            !settings_fn_src.contains("draw_liquid_glass"),
            "draw_settings_page must contain ZERO draw_liquid_glass calls"
        );

        ui.route = PageRoute::Home;
        ui.show_lyrics_overlay = false;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        // Glass hover/press springs must settle cleanly to 0% idle CPU
        renderer.step_glass_animations(1.0);
        assert!(
            !renderer.has_active_glass_motion(),
            "glass springs must settle to idle (has_active_glass_motion == false)"
        );
        let top_refresh_hits = ui
            .hit_regions
            .iter()
            .filter(|h| h.rect.y < 70.0 && matches!(h.action, UiAction::RefreshCurrentView))
            .count();
        assert_eq!(
            top_refresh_hits, 0,
            "Main window top-right floating nav must NOT contain a Refresh button"
        );

        unsafe {
            let _ = DestroyWindow(hwnd);
        }

        // 4. Lyrics view new defaults, toggles, queue wheel scroll, and end-of-playlist scroll-back latch
        let lv_def = LyricsViewState::default();
        assert!(lv_def.left_enlarged, "left_enlarged must default to true");
        assert!(!lv_def.center_axis, "center_axis must default to false");
        assert!(lv_def.show_lyrics, "show_lyrics must default to true");
        assert!(!lv_def.show_queue, "show_queue must default to false");

        // Persisted toggles & keeping NowPlaying context menu open
        ui.handle_lyrics_action(LyricsHitAction::OpenMoreMenu(RectF::new(400.0, 460.0, 32.0, 32.0)));
        assert!(matches!(
            ui.context_menu.as_ref().map(|c| &c.target),
            Some(ContextMenuTarget::NowPlaying)
        ));
        ui.execute_action(UiAction::ToggleLyricsLeftEnlarged);
        assert!(!ui.settings.lyrics_left_enlarged && !ui.lyrics_view.left_enlarged);
        assert!(ui.context_menu.is_some(), "toggling left_enlarged should keep NowPlaying menu open");
        ui.execute_action(UiAction::ToggleLyricsLeftEnlarged);
        assert!(ui.settings.lyrics_left_enlarged && ui.lyrics_view.left_enlarged);

        ui.execute_action(UiAction::ToggleLyricsCenterAxis);
        assert!(ui.settings.lyrics_center_axis && ui.lyrics_view.center_axis);
        ui.execute_action(UiAction::ToggleLyricsCenterAxis);
        assert!(!ui.settings.lyrics_center_axis && !ui.lyrics_view.center_axis);

        // ToggleLyricsVisible / ToggleLyricsQueue mutual interaction + queue wheel scroll
        ui.show_lyrics_overlay = true;
        ui.handle_lyrics_action(LyricsHitAction::ToggleLyricsQueue);
        assert!(ui.lyrics_view.show_queue);
        let mock_queue: Vec<AgentTrackInput> = (0..18)
            .map(|i| AgentTrackInput {
                id: format!("q_{i}"),
                name: format!("Queue Track {i}"),
                artist_name: "Test Artist".to_string(),
                album_name: "Test Album".to_string(),
                duration_sec: 210.0,
                ..Default::default()
            })
            .collect();
        ui.lyrics_view.queue_column_rect = RectF::new(720.0, 40.0, 620.0, 760.0);
        ui.lyrics_view.set_queue_tracks(mock_queue);
        assert!(ui.lyrics_view.queue_max_scroll_y > 100.0);
        ui.mouse_pos = (920.0, 360.0);
        ui.on_wheel(-1.0);
        assert!(
            ui.lyrics_view.queue_scroll_y > 20.0
                && ui.lyrics_view.queue_scroll_y <= ui.lyrics_view.queue_max_scroll_y,
            "mouse wheel over lyrics queue column must scroll queue_scroll_y: {}",
            ui.lyrics_view.queue_scroll_y
        );
        ui.on_wheel(10.0);
        assert_eq!(ui.lyrics_view.queue_scroll_y, 0.0);

        // Turning lyrics visible back ON closes queue cleanly
        ui.lyrics_view.show_lyrics = false;
        ui.handle_lyrics_action(LyricsHitAction::ToggleLyricsVisible);
        assert!(ui.lyrics_view.show_lyrics && !ui.lyrics_view.show_queue);

        // End-of-playlist lyrics scroll-back latch: stopping at end of song MUST NOT reset lyrics to line 0!
        let end_ttml = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
            <p begin="00:01.000" end="00:04.000"><span begin="00:01.000" end="00:04.000">Opening line</span></p>
            <p begin="00:06.000" end="00:10.000"><span begin="00:06.000" end="00:10.000">Middle chorus</span></p>
            <p begin="00:14.000" end="00:19.000"><span begin="00:14.000" end="00:19.000">Final farewell line</span></p>
        </div></body></tt>"#;
        let parsed_end = parse_ttml_lyrics(end_ttml, "TTML").expect("parse end ttml");
        let mut lv = LyricsViewState::default();
        // Play up to near the end of the final line (`t = 18.2s`, `is_playing = true`)
        for f in 0..45 {
            lv.step(
                Some(&parsed_end),
                false,
                "id:end_latch_track",
                17.5 + (f as f64) * 0.016,
                true,
                0.2,
                None,
                760.0,
                1.0 / 60.0,
                false,
                true,
            );
        }
        let end_line_y_before = lv.animator.line_states[2].y_spring.value;
        assert_eq!(lv.animator.primary_active_idx, Some(2));
        assert!(lv.end_latched_playhead.is_none());

        // Playlist ends: backend stops (`is_playing = false`) and snaps `playhead_sec` back to `0.0` on SAME track!
        for _ in 0..30 {
            lv.step(
                Some(&parsed_end),
                false,
                "id:end_latch_track",
                0.0,
                false,
                0.0,
                None,
                760.0,
                1.0 / 60.0,
                false,
                true,
            );
        }
        assert!(
            lv.end_latched_playhead.is_some(),
            "stopping at song end with playhead reset to 0.0 must engage end_latched_playhead"
        );
        assert!(
            lv.synced_playhead_sec > 15.0,
            "latched synced_playhead_sec must stay at song end instead of 0.0 (got {})",
            lv.synced_playhead_sec
        );
        assert_ne!(
            lv.animator.primary_active_idx,
            Some(0),
            "lyrics must NOT jump back to line 0 when playlist ends!"
        );
        assert!(
            (lv.animator.line_states[2].y_spring.value - end_line_y_before).abs() < 24.0,
            "final lyric line Y position must remain serenely near the end: before={end_line_y_before}, now={}",
            lv.animator.line_states[2].y_spring.value
        );

        // Resuming playback (`is_playing = true`) or calling `clear_end_latch()` clears the latch cleanly
        lv.clear_end_latch();
        assert!(lv.end_latched_playhead.is_none());
        lv.step(
            Some(&parsed_end),
            false,
            "id:end_latch_track",
            1.5,
            true,
            0.2,
            None,
            760.0,
            1.0 / 60.0,
            false,
            true,
        );
        assert_eq!(lv.animator.primary_active_idx, Some(0));
    }

    #[test]
    #[ignore]
    fn capture_v0211_qa_screenshots() {
        use am_light_lib::am_agent::AgentTrackInput;
        use am_light_lib::am_api;
        use am_light_lib::app_ui::{AppUi, PageRoute, UiAction};
        use am_light_lib::audio_viz::StreamAudioSpecs;
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::d2d_renderer::D2DRenderer;
        use am_light_lib::i18n::{set_active_language, AppLanguage};
        use am_light_lib::lyrics::parse_ttml_lyrics;
        use am_light_lib::lyrics_view::LyricsHitAction;
        use am_light_lib::smtc::SmtcState;
        use am_light_lib::theme::RectF;
        use std::path::PathBuf;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        unsafe extern "system" fn qa_wnd_proc(
            h: HWND,
            m: u32,
            w: WPARAM,
            l: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }

        let out_dir = std::env::var("AM_LIGHT_QA_OUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("am_light_v0211_qa"));
        std::fs::create_dir_all(&out_dir).expect("create QA screenshot dir");

        // Full window size 1400x880 @ 1.0x DPI, Dark theme, rebuilt iOS 26 Liquid Glass ON
        let w_px: u32 = 1400;
        let h_px: u32 = 880;
        let dpi_scale: f32 = 1.0;

        let hwnd = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = w!("AMLightQaCaptureHost");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(qa_wnd_proc),
                hInstance: hinst,
                lpszClassName: cls,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                cls,
                PCWSTR(w!("AM Light QA").as_ptr()),
                WS_POPUP,
                0,
                0,
                w_px as i32,
                h_px as i32,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("CreateWindowExW")
        };

        let mut renderer = D2DRenderer::new(hwnd, w_px, h_px, dpi_scale).expect("D2DRenderer::new");
        println!(
            "[QA FONT] has_bundled_inter_font={}, active_ui_font_family={:?}, mono_font_family={:?}",
            renderer.has_bundled_inter_font(),
            renderer.active_ui_font_family(),
            renderer.mono_font_family()
        );
        assert!(
            renderer.has_bundled_inter_font(),
            "bundled Inter font must load for QA screenshots"
        );
        let rt = tokio::runtime::Runtime::new().expect("tokio rt");

        set_active_language(AppLanguage::ZhCn);
        let shelves = rt
            .block_on(am_api::fetch_home_editorial_shelves("cn", None))
            .unwrap_or_default();

        let mut art_urls: Vec<(String, bool)> = Vec::new();
        for (si, shelf) in shelves.iter().take(4).enumerate() {
            for item in shelf.items.iter().take(10) {
                if !item.artwork_url.is_empty() {
                    art_urls.push((item.artwork_url.clone(), si == 0));
                }
            }
        }
        let fetched_arts: Vec<(String, bool, Vec<u8>)> = rt.block_on(async {
            let mut futs = Vec::new();
            for (u, hero) in art_urls {
                futs.push(async move {
                    let bytes = am_api::fetch_artwork_bytes(&u, hero).await.ok()?;
                    Some((u, hero, bytes))
                });
            }
            let mut out = Vec::new();
            for f in futs {
                if let Some(v) = f.await {
                    out.push(v);
                }
            }
            out
        });
        for (u, hero, bytes) in &fetched_arts {
            let _ = renderer.insert_artwork_bytes(u.clone(), bytes, *hero);
        }

        let available_items: Vec<_> = shelves
            .iter()
            .flat_map(|s| s.items.iter())
            .filter(|it| {
                !it.artwork_url.is_empty()
                    && (renderer.has_artwork(&it.artwork_url, false)
                        || renderer.has_artwork(&it.artwork_url, true))
            })
            .cloned()
            .collect();

        let hero_item = available_items.first().cloned();
        let lyrics_item = available_items.first().cloned();
        let hero_art_url = hero_item.as_ref().map(|it| it.artwork_url.clone());
        let lyrics_art_url = lyrics_item
            .as_ref()
            .map(|it| it.artwork_url.clone())
            .or_else(|| hero_art_url.clone());
        if let Some(ref u) = lyrics_art_url {
            if let Some((_, _, bytes)) = fetched_arts.iter().find(|(k, _, _)| k == u) {
                let _ = renderer.insert_artwork_bytes(u.clone(), bytes, true);
            }
        }

        let mut ui = AppUi::new(0);
        ui.settings.language = AppLanguage::ZhCn;
        ui.settings.storefront = "cn".to_string();
        ui.settings.user_token.clear();
        ui.settings.playback_backend = PlaybackBackend::AppleMusicNative;
        ui.settings.liquid_glass_enabled = true;
        ui.settings.reduce_transparency = false;
        ui.settings.window_corner_radius = 14.0;
        ui.settings.lyrics_free_blur = true;
        ui.settings.lyrics_immersive_mode = false;
        ui.settings.lyrics_left_enlarged = true;
        ui.settings.lyrics_center_axis = false;
        ui.settings.lyrics_auto_emphasis = true;
        ui.settings.lyrics_frontier_wave = true;
        ui.settings.lyrics_end_hold_glow = true;
        ui.lyrics_view.music_free_blur = true;
        ui.lyrics_view.left_enlarged = true;
        ui.lyrics_view.center_axis = false;
        ui.lyrics_view.show_lyrics = true;
        ui.lyrics_view.show_queue = false;
        ui.lyrics_view.show_translation = true;
        ui.official_account = Default::default();
        ui.home_shelves = shelves;

        let demo_specs = StreamAudioSpecs {
            codec: "alac".to_string(),
            codec_label: "ALAC".to_string(),
            tier_label: "高解析度无损".to_string(),
            sample_rate_hz: 96_000,
            bit_depth: Some(24),
            channels: "2ch".to_string(),
            bitrate_kbps: 3210,
            peak_bitrate_kbps: 3680,
            uncompressed_pcm_kbps: Some(4608),
            compression_ratio_pct: Some(69.7),
            atmos_fallback_reason: None,
            hires_catalog_trait: true,
            available_variants: Vec::new(),
        };

        let base_smtc = SmtcState {
            active: true,
            app_id: "AMPLibraryAgent.exe".to_string(),
            title: "Midnight Resonance".to_string(),
            artist: "Aurora Synth Ensemble".to_string(),
            album: "Liquid Glass Sessions (Hi-Res Master)".to_string(),
            status: "Playing".to_string(),
            position_sec: 60.4,
            duration_sec: 240.0,
            thumbnail_base64: None,
            artwork_url: hero_art_url.clone(),
            backend_mode: "agent".to_string(),
            audio_quality: Some("高解析度无损 · 24-bit / 96kHz".to_string()),
            volume: Some(0.75),
            agent_pid: None,
            adam_id: Some("1000001".to_string()),
            stream_specs: Some(demo_specs),
            sampled_at: Some(std::time::Instant::now()),
            timeline_updated_univ: 0,
            position_is_calibrated: true,
        };
        ui.on_playback_state_changed(base_smtc.clone());

        // ====================================================================
        // QA SHOT 1: Home Page with Rebuilt iOS 26 Liquid Glass ON (1400x880)
        // Scroll slightly so rich colorful covers sit behind the floating MiniPlayer!
        // ====================================================================
        ui.execute_action(UiAction::Navigate(PageRoute::Home));
        ui.show_lyrics_overlay = false;
        ui.scroll_y = 140.0;
        ui.scroll_target_y = 140.0;
        for _ in 0..24 {
            ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        }
        let s1 = out_dir.join("qa_1_home_liquid_glass.png");
        renderer.pending_capture_path = Some(s1.clone());
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        assert!(s1.exists(), "qa_1_home_liquid_glass.png must be written");

        // Prepare rich bilingual syllable-synced TTML lyrics around t = 60.4s (displays `1:00` and `-3:00` -> tests unslashed `0`!)
        let sample_ttml = concat!(
            r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">"#,
            r#"<head><metadata><iTunesMetadata><translations><translation xml:lang="zh-Hans-CN">"#,
            r#"<text for="L1">霓虹折射在午夜微凉的雨幕里</text>"#,
            r#"<text for="L2">每一道声波都在液态玻璃上缓缓绽放</text>"#,
            r#"<text for="L3">听见星海在无损高频间轻声回响</text>"#,
            r#"<text for="L4">让光影与旋律在此刻停留成永恒</text>"#,
            r#"<text for="L5">穿越云端与城市沉默的霓虹灯火</text>"#,
            r#"<text for="L6">闭上双眼感受律动在指尖轻轻起伏</text>"#,
            r#"</translation></translations></iTunesMetadata></metadata></head><body><div>"#,
            r#"<p begin="00:46.000" end="00:52.200" itunes:key="L1"><span begin="00:46.000" end="00:47.100">Neon</span> <span begin="00:47.100" end="00:48.200">lights</span> <span begin="00:48.200" end="00:49.400">dissolve</span> <span begin="00:49.400" end="00:50.600">across</span> <span begin="00:50.600" end="00:52.200">midnight rain</span></p>"#,
            r#"<p begin="00:52.600" end="00:58.400" itunes:key="L2"><span begin="00:52.600" end="00:53.500">Every</span> <span begin="00:53.500" end="00:54.500">wave</span> <span begin="00:54.500" end="00:55.700">unfolds</span> <span begin="00:55.700" end="00:56.800">in</span> <span begin="00:56.800" end="00:58.400" itunes:emphasis="true">liquid glass</span></p>"#,
            r#"<p begin="00:58.800" end="01:05.600" itunes:key="L3"><span begin="00:58.800" end="00:59.600">Hear</span> <span begin="00:59.600" end="01:00.200">the</span> <span begin="01:00.200" end="01:01.600" itunes:emphasis="true">starlight</span> <span begin="01:01.600" end="01:03.200" itunes:emphasis="true">shimmering</span> <span begin="01:03.200" end="01:04.400">through</span> <span begin="01:04.400" end="01:05.600">the air</span></p>"#,
            r#"<p begin="01:06.000" end="01:12.400" itunes:key="L4"><span begin="01:06.000" end="01:07.100">Hold</span> <span begin="01:07.100" end="01:08.100">this</span> <span begin="01:08.100" end="01:09.600" itunes:emphasis="true">melody</span> <span begin="01:09.600" end="01:11.000">inside</span> <span begin="01:11.000" end="01:12.400">the glow</span></p>"#,
            r#"<p begin="01:12.800" end="01:18.500" itunes:key="L5"><span begin="01:12.800" end="01:14.100">Drifting</span> <span begin="01:14.100" end="01:15.500">softly</span> <span begin="01:15.500" end="01:16.900">past</span> <span begin="01:16.900" end="01:18.500">the quiet sky</span></p>"#,
            r#"<p begin="01:18.900" end="01:24.500" itunes:key="L6"><span begin="01:18.900" end="01:20.200">Close</span> <span begin="01:20.200" end="01:21.500">your</span> <span begin="01:21.500" end="01:23.000">eyes</span> <span begin="01:23.000" end="01:24.500">and float</span></p>"#,
            r#"</div></body></tt>"#,
        );
        let parsed_lyr = parse_ttml_lyrics(sample_ttml, "Apple Music TTML").expect("sample ttml");
        let mut lyr_smtc = base_smtc.clone();
        lyr_smtc.artwork_url = lyrics_art_url.clone();
        lyr_smtc.position_sec = 60.4;
        lyr_smtc.duration_sec = 240.0;
        ui.on_playback_state_changed(lyr_smtc);
        ui.current_lyrics_key = "qa:v0211-lyrics".to_string();
        ui.current_lyrics = Some(parsed_lyr);
        ui.lyrics_loading = false;
        ui.show_lyrics_overlay = true;

        // ====================================================================
        // QA SHOT 2: Full-Screen Lyrics in 流光 Mode with left_enlarged = true
        // ====================================================================
        ui.settings.lyrics_free_blur = true;
        ui.lyrics_view.music_free_blur = true;
        ui.settings.lyrics_left_enlarged = true;
        ui.settings.lyrics_center_axis = false;
        ui.lyrics_view.show_lyrics = true;
        ui.lyrics_view.show_queue = false;
        ui.lyrics_view.show_translation = true;
        ui.context_menu = None;
        for f in 0..48 {
            ui.smtc.position_sec = 59.35 + (f as f64) * 0.015;
            ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        }
        ui.smtc.position_sec = 60.05;
        let s2 = out_dir.join("qa_2_lyrics_aurora_enlarged.png");
        renderer.pending_capture_path = Some(s2.clone());
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        assert!(s2.exists(), "qa_2_lyrics_aurora_enlarged.png must be written");

        // ====================================================================
        // QA SHOT 3: Full-Screen Lyrics with Left `•••` Context Menu OPEN
        // ====================================================================
        let dots_anchor = ui
            .lyrics_view
            .hit_zones
            .iter()
            .find(|z| matches!(z.action, LyricsHitAction::OpenMoreMenu(_)))
            .map(|z| z.rect)
            .unwrap_or_else(|| RectF::new(500.0, 560.0, 32.0, 32.0));
        ui.handle_lyrics_action(LyricsHitAction::OpenMoreMenu(dots_anchor));
        for _ in 0..8 {
            ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        }
        let s3 = out_dir.join("qa_3_lyrics_more_menu.png");
        renderer.pending_capture_path = Some(s3.clone());
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        assert!(s3.exists(), "qa_3_lyrics_more_menu.png must be written");

        // ====================================================================
        // QA SHOT 4: Full-Screen Lyrics with `center_axis = true`
        // ====================================================================
        ui.context_menu = None;
        ui.settings.lyrics_center_axis = true;
        ui.lyrics_view.center_axis = true;
        for _ in 0..24 {
            ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        }
        let s4 = out_dir.join("qa_4_lyrics_center_axis.png");
        renderer.pending_capture_path = Some(s4.clone());
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        assert!(s4.exists(), "qa_4_lyrics_center_axis.png must be written");

        // ====================================================================
        // QA SHOT 5: Full-Screen Lyrics with Right-Column `播放列表` Queue Open
        // ====================================================================
        ui.settings.lyrics_center_axis = false;
        ui.lyrics_view.center_axis = false;
        ui.play_next_queue.clear();
        let queue_titles = [
            ("Midnight Resonance", "Aurora Synth Ensemble", 240.0),
            ("Velvet Horizon", "Nocturne Collective", 210.0),
            ("Glass Reflections", "Kyoto Drift", 195.0),
            ("Solstice Rain", "Sora Takahashi", 268.0),
            ("Starlight Boulevard", "Chromatic Echo", 224.0),
            ("Amber Tide", "Pacific Waves", 180.0),
            ("Silent Frequency", "Lumina Studio", 252.0),
            ("Electric Dawn", "Synth Archive", 206.0),
            ("Cloudburst Suite", "Aether Quartet", 300.0),
            ("Neon Harbour", "Midnight Mono", 230.0),
            ("Crystal Reverie", "Opal Lines", 214.0),
            ("Afterglow 09", "Deep Horizon", 245.0),
        ];
        for (idx, &(title, artist, dur)) in queue_titles.iter().enumerate() {
            let art = available_items
                .get(idx % available_items.len().max(1))
                .map(|it| it.artwork_url.clone())
                .or_else(|| lyrics_art_url.clone());
            ui.play_next_queue.push_back(AgentTrackInput {
                id: if idx == 0 {
                    "1000001".to_string()
                } else {
                    format!("qa_q_{idx}")
                },
                catalog_id: None,
                kind: Some("song".to_string()),
                name: title.to_string(),
                artist_name: artist.to_string(),
                album_name: "Liquid Glass Sessions".to_string(),
                duration_sec: dur,
                artwork_url: art,
                audio_traits: vec!["hi-res-lossless".to_string()],
            });
        }
        ui.handle_lyrics_action(LyricsHitAction::ToggleLyricsQueue);
        for _ in 0..20 {
            ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        }
        let s5 = out_dir.join("qa_5_lyrics_queue_open.png");
        renderer.pending_capture_path = Some(s5.clone());
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, true);
        assert!(s5.exists(), "qa_5_lyrics_queue_open.png must be written");

        set_active_language(AppLanguage::Auto);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    #[ignore]
    fn capture_v0211_round2_hidpi_perf_qa() {
        use am_light_lib::am_api;
        use am_light_lib::app_ui::{AppUi, PageRoute, UiAction};
        use am_light_lib::audio_viz::{AudioOutputDevice, StreamAudioSpecs};
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::d2d_renderer::D2DRenderer;
        use am_light_lib::drm_fix::DrmDiagReport;
        use am_light_lib::i18n::{set_active_language, AppLanguage};
        use am_light_lib::lyrics::parse_ttml_lyrics;
        use am_light_lib::lyrics_view::LyricsHitAction;
        use am_light_lib::smtc::SmtcState;
        use am_light_lib::theme::RectF;
        use std::path::PathBuf;
        use std::time::Instant;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        unsafe extern "system" fn qa2_wnd_proc(
            h: HWND,
            m: u32,
            w: WPARAM,
            l: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }

        let make_host = |cls_name: windows::core::PCWSTR, w_px: u32, h_px: u32| -> HWND {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
                let wc = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(qa2_wnd_proc),
                    hInstance: hinst,
                    lpszClassName: cls_name,
                    ..Default::default()
                };
                let _ = RegisterClassW(&wc);
                CreateWindowExW(
                    WS_EX_NOREDIRECTIONBITMAP,
                    cls_name,
                    PCWSTR(w!("AM Light QA2").as_ptr()),
                    WS_POPUP,
                    0,
                    0,
                    w_px as i32,
                    h_px as i32,
                    None,
                    None,
                    Some(hinst),
                    None,
                )
                .expect("CreateWindowExW")
            }
        };

        let out_dir = std::env::var("AM_LIGHT_QA_OUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("am_light_v0211_qa2"));
        std::fs::create_dir_all(&out_dir).expect("create QA2 dir");

        let hwnd_100 = make_host(w!("AMLightQa2Host100"), 1400, 880);
        let hwnd_200 = make_host(w!("AMLightQa2Host200"), 2800, 1760);

        let mut r100 = D2DRenderer::new(hwnd_100, 1400, 880, 1.0).expect("D2DRenderer 100%");
        let mut r200 = D2DRenderer::new(hwnd_200, 2800, 1760, 2.0).expect("D2DRenderer 200%");
        assert!(r100.has_bundled_inter_font() && r100.verify_inter_formats_all_weights());
        assert!(r200.has_bundled_inter_font() && r200.verify_inter_formats_all_weights());

        let rt = tokio::runtime::Runtime::new().expect("tokio rt");
        set_active_language(AppLanguage::ZhCn);
        let shelves = rt
            .block_on(am_api::fetch_home_editorial_shelves("cn", None))
            .unwrap_or_default();

        let mut art_urls: Vec<(String, bool)> = Vec::new();
        for (si, shelf) in shelves.iter().take(4).enumerate() {
            for item in shelf.items.iter().take(10) {
                if !item.artwork_url.is_empty() {
                    art_urls.push((item.artwork_url.clone(), si == 0));
                }
            }
        }
        let fetched_arts: Vec<(String, bool, Vec<u8>)> = rt.block_on(async {
            let mut futs = Vec::new();
            for (u, hero) in art_urls {
                futs.push(async move {
                    let bytes = am_api::fetch_artwork_bytes(&u, hero).await.ok()?;
                    Some((u, hero, bytes))
                });
            }
            let mut out = Vec::new();
            for f in futs {
                if let Some(v) = f.await {
                    out.push(v);
                }
            }
            out
        });
        for (u, hero, bytes) in &fetched_arts {
            let _ = r100.insert_artwork_bytes(u.clone(), bytes, *hero);
            let _ = r200.insert_artwork_bytes(u.clone(), bytes, *hero);
        }

        let hero_art_url = shelves
            .iter()
            .flat_map(|s| s.items.iter())
            .find(|it| {
                !it.artwork_url.is_empty()
                    && (r100.has_artwork(&it.artwork_url, false)
                        || r100.has_artwork(&it.artwork_url, true))
            })
            .map(|it| it.artwork_url.clone());
        if let Some(ref u) = hero_art_url {
            if let Some((_, _, bytes)) = fetched_arts.iter().find(|(k, _, _)| k == u) {
                let _ = r100.insert_artwork_bytes(u.clone(), bytes, true);
                let _ = r200.insert_artwork_bytes(u.clone(), bytes, true);
            }
        }

        let mut ui = AppUi::new(0);
        ui.settings.language = AppLanguage::ZhCn;
        ui.settings.storefront = "cn".to_string();
        ui.settings.user_token.clear();
        ui.settings.playback_backend = PlaybackBackend::AppleMusicNative;
        ui.settings.liquid_glass_enabled = true;
        ui.settings.reduce_transparency = false;
        ui.settings.window_corner_radius = 14.0;
        ui.settings.lyrics_free_blur = true;
        ui.settings.lyrics_immersive_mode = false;
        ui.settings.lyrics_left_enlarged = true;
        ui.settings.lyrics_center_axis = false;
        ui.lyrics_view.music_free_blur = true;
        ui.lyrics_view.left_enlarged = true;
        ui.lyrics_view.center_axis = false;
        ui.lyrics_view.show_lyrics = true;
        ui.lyrics_view.show_queue = false;
        ui.lyrics_view.show_translation = true;
        ui.official_account = Default::default();
        ui.drm_diag = DrmDiagReport {
            checked: true,
            healthy: true,
            adi_dir_exists: true,
            adi_file_count: 1,
            unwritable_files: Vec::new(),
            sc_info_unwritable: false,
            hklm_redirections: Vec::new(),
            summary: "DRM 授权状态正常（1 个 ADI 身份文件均可正常读写）".to_string(),
            detail: "设备授权正常，支持无损与高解析度流媒体播放".to_string(),
        };
        ui.devices = vec![AudioOutputDevice {
            id: "studio-dac".to_string(),
            name: "Studio DAC USB Audio".to_string(),
            is_default: true,
            sample_rate: 96_000,
            bit_depth: 24,
            channels: 2,
            format_tag: "PCM".to_string(),
            spatial_audio_active: false,
        }];
        ui.current_monitor_hz = 144;
        ui.home_shelves = shelves;

        let demo_specs = StreamAudioSpecs {
            codec: "alac".to_string(),
            codec_label: "ALAC".to_string(),
            tier_label: "高解析度无损".to_string(),
            sample_rate_hz: 96_000,
            bit_depth: Some(24),
            channels: "2ch".to_string(),
            bitrate_kbps: 3210,
            peak_bitrate_kbps: 3680,
            uncompressed_pcm_kbps: Some(4608),
            compression_ratio_pct: Some(69.7),
            atmos_fallback_reason: None,
            hires_catalog_trait: true,
            available_variants: Vec::new(),
        };
        let base_smtc = SmtcState {
            active: true,
            app_id: "AMPLibraryAgent.exe".to_string(),
            title: "Midnight Resonance".to_string(),
            artist: "Aurora Synth Ensemble".to_string(),
            album: "Liquid Glass Sessions (Hi-Res Master)".to_string(),
            status: "Playing".to_string(),
            position_sec: 60.05,
            duration_sec: 240.0,
            thumbnail_base64: None,
            artwork_url: hero_art_url.clone(),
            backend_mode: "agent".to_string(),
            audio_quality: Some("高解析度无损 · 24-bit / 96kHz".to_string()),
            volume: Some(0.75),
            agent_pid: None,
            adam_id: Some("1000001".to_string()),
            stream_specs: Some(demo_specs),
            sampled_at: Some(Instant::now()),
            timeline_updated_univ: 0,
            position_is_calibrated: true,
        };
        ui.on_playback_state_changed(base_smtc.clone());

        let bench_30 = |label: &str, ui: &mut AppUi, r: &mut D2DRenderer| {
            // Warmup 6 frames so caches & GPU pipelines stabilize
            for _ in 0..6 {
                ui.step_and_render_ex(r, 1.0 / 60.0, true);
            }
            let mut min_ms = f64::MAX;
            let mut max_ms = 0.0f64;
            let mut sum_ms = 0.0f64;
            for _ in 0..30 {
                let t0 = Instant::now();
                ui.step_and_render_ex(r, 1.0 / 60.0, true);
                let ms = t0.elapsed().as_secs_f64() * 1000.0;
                min_ms = min_ms.min(ms);
                max_ms = max_ms.max(ms);
                sum_ms += ms;
            }
            let avg_ms = sum_ms / 30.0;
            println!(
                "[PERF] {label}: avg={avg_ms:.2} ms/frame (min={min_ms:.2} ms, max={max_ms:.2} ms, ~{:.0} FPS)",
                1000.0 / avg_ms.max(0.01)
            );
        };

        // ====================================================================
        // SHOT 1: Home @ 100% DPI (1400x880) with Ultra-Clear Vertical-Rim Liquid Glass
        // ====================================================================
        ui.execute_action(UiAction::Navigate(PageRoute::Home));
        ui.show_lyrics_overlay = false;
        ui.scroll_y = 140.0;
        ui.scroll_target_y = 140.0;
        bench_30("Home (Liquid Glass ON, 100% DPI 1400x880)", &mut ui, &mut r100);
        let p1 = out_dir.join("qa2_1_home_100dpi.png");
        r100.pending_capture_path = Some(p1.clone());
        ui.step_and_render_ex(&mut r100, 1.0 / 60.0, true);
        assert!(p1.exists(), "qa2_1_home_100dpi.png must be written");

        // ====================================================================
        // SHOT 2: Home @ 200% HiDPI (2800x1760 physical px) — verify ZERO seam/banding!
        // ====================================================================
        bench_30("Home (Liquid Glass ON, 200% HiDPI 2800x1760)", &mut ui, &mut r200);
        let p2 = out_dir.join("qa2_2_home_200hidpi.png");
        r200.pending_capture_path = Some(p2.clone());
        ui.step_and_render_ex(&mut r200, 1.0 / 60.0, true);
        assert!(p2.exists(), "qa2_2_home_200hidpi.png must be written");

        // ====================================================================
        // SHOT 3: New iPadOS Grouped Settings Page (ZERO Liquid Glass!)
        // ====================================================================
        ui.execute_action(UiAction::Navigate(PageRoute::Settings));
        ui.show_lyrics_overlay = false;
        ui.scroll_y = 0.0;
        ui.scroll_target_y = 0.0;
        ui.banner_text = None;
        assert!(ui.glass_off(), "Settings must run with glass_off == true");
        bench_30("Settings (iPadOS matte, 100% DPI 1400x880)", &mut ui, &mut r100);
        bench_30("Settings (iPadOS matte, 200% HiDPI 2800x1760)", &mut ui, &mut r200);
        let p3 = out_dir.join("qa2_3_settings_ipados.png");
        r100.pending_capture_path = Some(p3.clone());
        ui.step_and_render_ex(&mut r100, 1.0 / 60.0, true);
        assert!(p3.exists(), "qa2_3_settings_ipados.png must be written");

        // Also capture scrolled middle/lower Settings sections for visual audit
        ui.scroll_y = 560.0;
        ui.scroll_target_y = 560.0;
        for _ in 0..8 {
            ui.step_and_render_ex(&mut r100, 1.0 / 60.0, true);
        }
        let p3b = out_dir.join("qa2_3b_settings_ipados_mid.png");
        r100.pending_capture_path = Some(p3b.clone());
        ui.step_and_render_ex(&mut r100, 1.0 / 60.0, true);
        assert!(p3b.exists());

        // ====================================================================
        // SHOT 4: Lyrics Page in 流光 Mode + Clear Vertical-Rim Liquid Glass `•••` Menu
        // ====================================================================
        let sample_ttml = concat!(
            r#"<tt xmlns="http://www.w3.org/ns/ttml" xmlns:itunes="http://music.apple.com/lyric-ttml-internal" xmlns:ttm="http://www.w3.org/ns/ttml#metadata">"#,
            r#"<head><metadata><iTunesMetadata><translations><translation xml:lang="zh-Hans-CN">"#,
            r#"<text for="L1">霓虹折射在午夜微凉的雨幕里</text>"#,
            r#"<text for="L2">每一道声波都在液态玻璃上缓缓绽放</text>"#,
            r#"<text for="L3">听见星海在无损高频间轻声回响</text>"#,
            r#"<text for="L4">让光影与旋律在此刻停留成永恒</text>"#,
            r#"<text for="L5">穿越云端与城市沉默的霓虹灯火</text>"#,
            r#"<text for="L6">闭上双眼感受律动在指尖轻轻起伏</text>"#,
            r#"</translation></translations></iTunesMetadata></metadata></head><body><div>"#,
            r#"<p begin="00:46.000" end="00:52.200" itunes:key="L1"><span begin="00:46.000" end="00:47.100">Neon</span> <span begin="00:47.100" end="00:48.200">lights</span> <span begin="00:48.200" end="00:49.400">dissolve</span> <span begin="00:49.400" end="00:50.600">across</span> <span begin="00:50.600" end="00:52.200">midnight rain</span></p>"#,
            r#"<p begin="00:52.600" end="00:58.400" itunes:key="L2"><span begin="00:52.600" end="00:53.500">Every</span> <span begin="00:53.500" end="00:54.500">wave</span> <span begin="00:54.500" end="00:55.700">unfolds</span> <span begin="00:55.700" end="00:56.800">in</span> <span begin="00:56.800" end="00:58.400" itunes:emphasis="true">liquid glass</span></p>"#,
            r#"<p begin="00:58.800" end="01:05.600" itunes:key="L3"><span begin="00:58.800" end="00:59.600">Hear</span> <span begin="00:59.600" end="01:00.200">the</span> <span begin="01:00.200" end="01:01.600" itunes:emphasis="true">starlight</span> <span begin="01:01.600" end="01:03.200" itunes:emphasis="true">shimmering</span> <span begin="01:03.200" end="01:04.400">through</span> <span begin="01:04.400" end="01:05.600">the air</span></p>"#,
            r#"<p begin="01:06.000" end="01:12.400" itunes:key="L4"><span begin="01:06.000" end="01:07.100">Hold</span> <span begin="01:07.100" end="01:08.100">this</span> <span begin="01:08.100" end="01:09.600" itunes:emphasis="true">melody</span> <span begin="01:09.600" end="01:11.000">inside</span> <span begin="01:11.000" end="01:12.400">the glow</span></p>"#,
            r#"<p begin="01:12.800" end="01:18.500" itunes:key="L5"><span begin="01:12.800" end="01:14.100">Drifting</span> <span begin="01:14.100" end="01:15.500">softly</span> <span begin="01:15.500" end="01:16.900">past</span> <span begin="01:16.900" end="01:18.500">the quiet sky</span></p>"#,
            r#"<p begin="01:18.900" end="01:24.500" itunes:key="L6"><span begin="01:18.900" end="01:20.200">Close</span> <span begin="01:20.200" end="01:21.500">your</span> <span begin="01:21.500" end="01:23.000">eyes</span> <span begin="01:23.000" end="01:24.500">and float</span></p>"#,
            r#"</div></body></tt>"#,
        );
        let parsed_lyr = parse_ttml_lyrics(sample_ttml, "Apple Music TTML").expect("sample ttml");
        ui.current_lyrics_key = "qa2:lyrics".to_string();
        ui.current_lyrics = Some(parsed_lyr);
        ui.lyrics_loading = false;
        ui.show_lyrics_overlay = true;
        ui.context_menu = None;
        for f in 0..30 {
            ui.smtc.position_sec = 59.60 + (f as f64) * 0.015;
            ui.step_and_render_ex(&mut r100, 1.0 / 60.0, true);
        }
        bench_30("Lyrics (流光 Aurora, 100% DPI 1400x880)", &mut ui, &mut r100);

        let dots_anchor = ui
            .lyrics_view
            .hit_zones
            .iter()
            .find(|z| matches!(z.action, LyricsHitAction::OpenMoreMenu(_)))
            .map(|z| z.rect)
            .unwrap_or_else(|| RectF::new(500.0, 560.0, 32.0, 32.0));
        ui.handle_lyrics_action(LyricsHitAction::OpenMoreMenu(dots_anchor));
        // Let popover morph spring settle fully open
        for _ in 0..24 {
            ui.step_and_render_ex(&mut r100, 1.0 / 60.0, true);
        }
        let p4 = out_dir.join("qa2_4_lyrics_menu_glass.png");
        r100.pending_capture_path = Some(p4.clone());
        ui.step_and_render_ex(&mut r100, 1.0 / 60.0, true);
        assert!(p4.exists(), "qa2_4_lyrics_menu_glass.png must be written");

        set_active_language(AppLanguage::Auto);
        unsafe {
            let _ = DestroyWindow(hwnd_100);
            let _ = DestroyWindow(hwnd_200);
        }
    }

    #[test]
    fn test_v0212_am_playlist_pagination_and_fast_search_overhaul() {
        use am_light_lib::am_agent::AgentTrackInput;
        use am_light_lib::am_api::{
            build_next_amp_path, build_offset_amp_path, parse_next_offset,
        };
        use am_light_lib::app_ui::{
            AppUi, MediaCardItem, PageRoute, PlaylistDiskCache, SearchCatalogPayload, UiAction,
            PLAYLIST_CACHE_SCHEMA_VERSION,
        };

        // 1. Apple Music pagination URL helpers preserve query params (`include`, `extend`, `limit`, `l`)
        let init_tracks =
            "v1/me/library/playlists/p.12345/tracks?include=catalog&extend=audioTraits&limit=100&l=zh-Hans-CN";
        let next_tracks =
            build_next_amp_path(init_tracks, "/v1/me/library/playlists/p.12345/tracks?offset=100");
        assert!(next_tracks.starts_with("v1/me/library/playlists/p.12345/tracks?"));
        assert!(next_tracks.contains("offset=100"));
        assert!(next_tracks.contains("include=catalog"));
        assert!(next_tracks.contains("extend=audioTraits"));
        assert!(next_tracks.contains("limit=100"));
        assert!(next_tracks.contains("l=zh-Hans-CN"));
        assert_eq!(parse_next_offset(&next_tracks), Some(100));
        assert_eq!(
            parse_next_offset("/v1/me/library/playlists?cursor=abc123"),
            None
        );

        let off_path = build_offset_amp_path(init_tracks, 300);
        assert!(off_path.contains("offset=300"));
        assert!(off_path.contains("include=catalog"));
        assert!(off_path.contains("limit=100"));
        assert_eq!(off_path.matches("offset=").count(), 1);

        // 2. PlaylistDiskCache schema version & complete >24/>40 playlist retention
        let legacy: PlaylistDiskCache =
            serde_json::from_str(r#"{"sidebar_playlists":[],"details":{},"last_sync_unix_sec":1}"#)
                .expect("legacy playlist cache deserialize");
        assert_eq!(legacy.schema_version, 0);

        let mut many_pls: Vec<MediaCardItem> = Vec::new();
        for i in 0..85 {
            many_pls.push(MediaCardItem {
                id: format!("p.pl_{i}"),
                kind: "library-playlists".to_string(),
                title: if i == 60 {
                    "周杰伦经典全集".to_string()
                } else {
                    format!("我的苹果歌单 #{i}")
                },
                subtitle: "资料库播放列表".to_string(),
                description: String::new(),
                artwork_url: None,
                badge: None,
                track_input: None,
            });
        }
        let mut cache = PlaylistDiskCache::default();
        cache.upsert_sidebar(&many_pls, 1_700_000_000);
        assert_eq!(cache.schema_version, PLAYLIST_CACHE_SCHEMA_VERSION);
        assert_eq!(cache.sidebar_playlists.len(), 85);

        // 3. Search zero-redraw typing, instant local matches, 0ms LRU cache hit, and stale seq guard
        let mut ui = AppUi::new(0);
        ui.sidebar_playlists = many_pls;
        ui.lib_songs.push(AgentTrackInput {
            id: "1001".to_string(),
            catalog_id: Some("1001".to_string()),
            kind: Some("songs".to_string()),
            name: "晴天".to_string(),
            artist_name: "周杰伦".to_string(),
            album_name: "叶惠美".to_string(),
            duration_sec: 269.0,
            artwork_url: None,
            audio_traits: vec!["lossless".to_string()],
        });

        ui.search_focused = true;
        for ch in "周杰伦".chars() {
            assert!(ui.on_char_input(ch));
        }
        assert_eq!(ui.search_query, "周杰伦");
        assert!(ui.last_search_typed.is_some());
        // Typing debounce must NEVER trigger continuous full-window redraws!
        assert!(
            !ui.needs_continuous_animation(),
            "typing in search box must NOT cause continuous idle redraw storms"
        );
        // Instant 0ms local library matches find playlist #60 (>24 cap!) and local song!
        assert!(
            ui.search_local_cards.iter().any(|c| c.id == "p.pl_60"),
            "instant local search must find playlist beyond old 24/40 cap"
        );
        assert!(
            ui.search_local_songs.iter().any(|s| s.name == "晴天"),
            "instant local search must match library tracks"
        );

        // Populate search LRU cache and verify 0ms hit when typing/editing back to cached query
        let cache_key = format!(
            "{:?}:{}:{}",
            ui.settings.playback_backend,
            ui.settings.storefront.to_ascii_lowercase(),
            "周杰伦"
        );
        ui.put_search_cache(
            cache_key,
            SearchCatalogPayload {
                artists: vec![MediaCardItem {
                    id: "300117743".to_string(),
                    kind: "artists".to_string(),
                    title: "周杰伦".to_string(),
                    subtitle: "艺人".to_string(),
                    description: String::new(),
                    artwork_url: None,
                    badge: None,
                    track_input: None,
                }],
                albums: Vec::new(),
                playlists: Vec::new(),
                songs: vec![ui.lib_songs[0].clone()],
            },
        );
        // Add a char then backspace back to "周杰伦" -> instant 0ms cache hit!
        assert!(ui.on_char_input('a'));
        assert!(ui.on_char_input('\u{08}'));
        assert_eq!(ui.search_query, "周杰伦");
        assert!(!ui.search_loading);
        assert!(ui.last_search_typed.is_none());
        assert_eq!(ui.search_artists.len(), 1);
        assert_eq!(ui.search_songs.len(), 1);

        // ClearSearch resets all search caches/state cleanly
        ui.execute_action(UiAction::ClearSearch);
        assert!(ui.search_query.is_empty());
        assert!(ui.search_char_xs_query.is_empty());
        assert!(ui.search_artists.is_empty());
        assert!(ui.search_songs.is_empty());
        assert!(ui.search_local_cards.is_empty());
        assert!(!ui.search_loading);
        assert!(ui.route != PageRoute::Settings);
    }

    #[test]
    fn test_v0212_windows_smtc_publisher_timeline_commands_and_self_loop_guard() {
        use am_light_lib::app_ui::{AppSettings, AppUi, UiAction};
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::smtc::{
            self, compute_timeline_should_update, is_self_aumid, SmtcPublishPayload,
            SmtcPublishedCommand, SmtcState,
        };
        use windows::core::w;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPEDWINDOW,
        };

        // 1. Legacy AppSettings JSON backward compatibility: `smtc_publish_enabled` defaults to `true`
        let legacy: AppSettings = serde_json::from_str(r#"{"storefront":"cn"}"#)
            .expect("legacy settings deserialize");
        assert!(legacy.smtc_publish_enabled);

        // 2. Self-AUMID defense-in-depth guard prevents SMTC consumer from binding to AM-Light itself
        assert!(is_self_aumid("am-light.exe"));
        assert!(is_self_aumid("c:\\apps\\amlight\\am-light.exe"));
        assert!(!is_self_aumid("appleinc.applemusicwin_nzyj5cx40ttqa!app"));

        // 3. Deterministic timeline sync policy (essential for Lyricify Fusion accuracy without spam)
        assert!(compute_timeline_should_update(
            false, true, false, true, 0.0, 210.0, 0.0, 210.0, 0.1
        )); // track changed
        assert!(compute_timeline_should_update(
            false, false, true, false, 45.0, 210.0, 45.0, 210.0, 0.1
        )); // status changed
        assert!(compute_timeline_should_update(
            true, false, false, true, 45.0, 210.0, 44.9, 210.0, 0.1
        )); // forced seek
        assert!(compute_timeline_should_update(
            false, false, false, true, 46.8, 210.0, 45.0, 210.0, 1.0
        )); // drift >0.35s
        assert!(compute_timeline_should_update(
            false, false, false, true, 50.1, 210.0, 45.0, 210.0, 5.1
        )); // gentle 5s heartbeat
        assert!(!compute_timeline_should_update(
            false, false, false, true, 46.05, 210.0, 45.0, 210.0, 1.0
        )); // smooth <0.35s playback -> no IPC spam!
        assert!(!compute_timeline_should_update(
            false, false, false, false, 45.05, 210.0, 45.0, 210.0, 10.0
        )); // paused without seek -> no spam!
        assert!(compute_timeline_should_update(
            false, false, false, false, 52.0, 210.0, 45.0, 210.0, 0.5
        )); // paused user seek -> immediate sync!

        // 4. Backend self-loop guard & Settings toggle in AppUi
        let mut ui = AppUi::new(0);
        ui.settings.smtc_publish_enabled = true;
        ui.settings.playback_backend = PlaybackBackend::AppleMusicNative;
        ui.smtc = SmtcState {
            active: true,
            title: "晴天".to_string(),
            artist: "周杰伦".to_string(),
            album: "叶惠美".to_string(),
            status: "Playing".to_string(),
            position_sec: 32.5,
            duration_sec: 269.0,
            backend_mode: "agent".to_string(),
            ..SmtcState::default()
        };
        assert!(ui.should_publish_smtc());

        // Passive external Apple Music SMTC mirror must NOT duplicate-publish
        ui.smtc.backend_mode = "smtc".to_string();
        assert!(!ui.should_publish_smtc());

        // NetEase / QQ Music / Web AM built-in backends DO publish
        ui.settings.playback_backend = PlaybackBackend::NetEase;
        ui.smtc.backend_mode = "netease".to_string();
        assert!(ui.should_publish_smtc());
        ui.settings.playback_backend = PlaybackBackend::QQMusic;
        ui.smtc.backend_mode = "qqmusic".to_string();
        assert!(ui.should_publish_smtc());
        ui.settings.playback_backend = PlaybackBackend::AppleMusicWeb;
        ui.smtc.backend_mode = "web-am".to_string();
        assert!(ui.should_publish_smtc());

        // ExternalSmtc backend MUST disable publishing to prevent self-loop
        ui.settings.playback_backend = PlaybackBackend::ExternalSmtc;
        ui.smtc.backend_mode = "agent".to_string();
        assert!(!ui.should_publish_smtc());

        // ToggleSmtcPublish flips setting cleanly
        ui.settings.playback_backend = PlaybackBackend::AppleMusicNative;
        assert!(ui.should_publish_smtc());
        ui.execute_action(UiAction::ToggleSmtcPublish);
        assert!(!ui.settings.smtc_publish_enabled);
        assert!(!ui.should_publish_smtc());
        ui.execute_action(UiAction::ToggleSmtcPublish);
        assert!(ui.settings.smtc_publish_enabled);
        assert!(ui.should_publish_smtc());

        // 5. Media button / seek command queue round-trip
        let _ = smtc::drain_published_commands();
        smtc::enqueue_published_command(SmtcPublishedCommand::Pause);
        smtc::enqueue_published_command(SmtcPublishedCommand::Next);
        smtc::enqueue_published_command(SmtcPublishedCommand::SeekSec(42.5));
        let cmds = smtc::drain_published_commands();
        assert_eq!(
            cmds,
            vec![
                SmtcPublishedCommand::Pause,
                SmtcPublishedCommand::Next,
                SmtcPublishedCommand::SeekSec(42.5),
            ]
        );
        assert!(smtc::drain_published_commands().is_empty());

        // 6. Live WinRT `ISystemMediaTransportControlsInterop::GetForWindow` + Thumbnail Stream test
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("AM-Light-SMTC-Test"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                100,
                100,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .expect("create hidden test window");

            let inited = smtc::init_publisher(hwnd.0 as usize);
            assert!(inited, "ISystemMediaTransportControlsInterop::GetForWindow must succeed");
            assert!(!smtc::is_publisher_enabled(), "publisher starts disabled when idle");

            let payload = SmtcPublishPayload {
                title: "稻香".to_string(),
                artist: "周杰伦".to_string(),
                album: "魔杰座".to_string(),
                status: "Playing".to_string(),
                position_sec: 12.345,
                duration_sec: 223.0,
                track_key: "稻香\x1f周杰伦\x1f魔杰座".to_string(),
            };
            let need_thumb = smtc::sync_publisher_state(&payload, true);
            assert!(need_thumb);
            assert!(smtc::is_publisher_enabled());

            // Minimal valid 1x1 PNG bytes for InMemoryRandomAccessStream thumbnail upload
            const TINY_PNG: &[u8] = &[
                0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49,
                0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02,
                0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44,
                0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01,
                0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44,
                0xAE, 0x42, 0x60, 0x82,
            ];
            let thumb_ok = smtc::set_publisher_thumbnail_bytes(&payload.track_key, TINY_PNG);
            assert!(thumb_ok, "WinRT InMemoryRandomAccessStream thumbnail upload must succeed");
            // Second sync for the same track does not re-request thumbnail
            let need_thumb_again = smtc::sync_publisher_state(&payload, false);
            assert!(!need_thumb_again);

            // Disable & clean shutdown
            smtc::disable_publisher();
            assert!(!smtc::is_publisher_enabled());
            smtc::shutdown_publisher();
            let _ = DestroyWindow(hwnd);
        }

        // 7. Rolling runtime & crash logger path validation
        let log_p = am_light_lib::logger::log_file_path().expect("log file path");
        let crash_p = am_light_lib::logger::crash_log_path().expect("crash log path");
        assert!(log_p.to_string_lossy().ends_with("amlight.log"));
        assert!(crash_p.to_string_lossy().ends_with("crash_latest.log"));
    }

    #[test]
    fn test_performance_investigation_optimizations() {
        use am_light_lib::am_agent::AgentTrackInput;
        use am_light_lib::app_ui::{AppUi, FpsCap, PageRoute, UiAction};
        use am_light_lib::audio_viz::{is_viz_subscribed, set_viz_sub_mode, viz_sub_mode, VizSubMode};
        use am_light_lib::backends::ResolvedAudioStream;
        use am_light_lib::i18n::{translate_ui_text_for, ResolvedLang};
        use am_light_lib::lyrics::{parse_yrc_lyrics, LyricsAnimator};
        use am_light_lib::native_player::inspect_audio_file_specs;
        use std::borrow::Cow;
        use std::sync::Arc;
        use std::time::Instant;

        // 1. P0-1: inspect_audio_file_specs reads ONLY 64 bytes (zero full-file allocation on large FLAC!)
        let tmp_flac = std::env::temp_dir().join(format!(
            "am_light_perf_flac_{}.flac",
            std::process::id()
        ));
        {
            let mut buf = vec![0u8; 4 * 1024 * 1024]; // 4 MiB synthetic FLAC
            buf[0..4].copy_from_slice(b"fLaC");
            buf[4] = 0x80; // last metadata block, type 0 = STREAMINFO
            buf[7] = 34; // STREAMINFO length
            // Encode 96,000 Hz (20-bit: 0x17700), 2 ch (ch-1 = 1 -> 0b001), 24-bit (bps-1 = 23 -> 0b10111)
            // byte[18] = sr[19..12] = 0x17
            // byte[19] = sr[11..4]  = 0x70
            // byte[20] = sr[3..0]<<4 | (ch-1)<<1 | (bps-1)>>4 = 0x00 | 0x02 | 0x01 = 0x03
            // byte[21] = ((bps-1)&0x0F)<<4 = 0x70
            buf[18] = 0x17;
            buf[19] = 0x70;
            buf[20] = 0x03;
            buf[21] = 0x70;
            std::fs::write(&tmp_flac, &buf).expect("write temp flac");
        }
        let dummy_stream = ResolvedAudioStream {
            url: "https://example.invalid/audio.flac".to_string(),
            file_ext: "flac".to_string(),
            quality_label: "Hi-Res FLAC".to_string(),
            nominal_bitrate_kbps: 0,
            is_trial: false,
            referer: String::new(),
            cookie: None,
        };
        let t0 = Instant::now();
        let mut last_spec = None;
        for _ in 0..50 {
            last_spec = Some(inspect_audio_file_specs(&tmp_flac, &dummy_stream, 44100, 2, 120.0));
        }
        let elapsed_ms = t0.elapsed().as_millis();
        let _ = std::fs::remove_file(&tmp_flac);
        let spec = last_spec.expect("spec");
        assert_eq!(spec.codec, "FLAC");
        assert_eq!(spec.sample_rate_hz, 96_000);
        assert_eq!(spec.bit_depth, Some(24));
        assert!(spec.codec_label.contains("FLAC"));
        assert!(
            elapsed_ms < 250,
            "50x 64-byte header inspections on 4 MiB file must stay fast (took {elapsed_ms}ms)"
        );

        // 2. P0-3: logger::has_recent_crash_log() is atomic-cached (zero per-frame syscalls)
        let _ = am_light_lib::logger::has_recent_crash_log();
        let t_log = Instant::now();
        for _ in 0..10_000 {
            let _ = std::hint::black_box(am_light_lib::logger::has_recent_crash_log());
        }
        assert!(
            t_log.elapsed().as_millis() < 20,
            "10,000 cached has_recent_crash_log() calls must complete in <20ms"
        );

        // 3. P0-4: 3-tier VizSubMode (Off / MiniBars5 / FullVisualizer)
        set_viz_sub_mode(VizSubMode::Off);
        let mut ui = AppUi::new(0);
        ui.route = PageRoute::Home;
        ui.show_lyrics_overlay = false;
        ui.smtc.status = "Paused".to_string();
        ui.update_viz_subscription();
        assert_eq!(viz_sub_mode(), VizSubMode::Off);
        assert!(!is_viz_subscribed());

        ui.smtc.status = "Playing".to_string();
        ui.settings.live_eq_icon_spectrum = true;
        ui.settings.reduce_motion = false;
        ui.update_viz_subscription();
        assert_eq!(
            viz_sub_mode(),
            VizSubMode::MiniBars5,
            "Home playing with live EQ icon must subscribe lightweight MiniBars5, NOT FullVisualizer"
        );
        assert!(is_viz_subscribed());

        ui.route = PageRoute::Visualizer;
        ui.update_viz_subscription();
        assert_eq!(viz_sub_mode(), VizSubMode::FullVisualizer);

        ui.show_lyrics_overlay = true;
        ui.update_viz_subscription();
        assert_eq!(viz_sub_mode(), VizSubMode::Off);
        set_viz_sub_mode(VizSubMode::Off);

        // 4. P0-2: Cover Flow runs at full VSync (not 30 FPS), paused mouse over blank content does NOT dirty,
        //          and unfocused Visualizer VSync throttles cleanly to Adaptive 60/72Hz
        ui.show_lyrics_overlay = false;
        ui.smtc.status = "Paused".to_string();
        ui.current_monitor_hz = 144;
        ui.is_window_focused = true;
        ui.route = PageRoute::LibAlbums;
        ui.albums_cover_flow = true;
        ui.cover_flow_index = 0.0;
        ui.cover_flow_target = 3;
        assert!(
            ui.has_active_ui_motion(),
            "Cover Flow settling must trigger high-refresh UI motion"
        );
        assert_eq!(
            ui.effective_frame_pacing().1,
            144,
            "Cover Flow must run at full 144Hz VSync instead of 30 FPS"
        );

        ui.albums_cover_flow = false;
        ui.route = PageRoute::Home;
        ui.settings.liquid_glass_enabled = true;
        ui.top_bar_guard_spring.snap(0.0);
        ui.hit_regions.clear();
        let _ = ui.on_mouse_move(400.0, 400.0); // prime cursor outside top bar
        assert!(
            !ui.on_mouse_move(420.0, 410.0),
            "Paused pointer motion over non-interactive content with Liquid Glass ON must NOT dirty idle frame"
        );
        assert!(
            ui.on_mouse_move(420.0, 20.0),
            "Moving pointer into top bar must wake top-bar guard & glass specular cleanly"
        );

        ui.route = PageRoute::Visualizer;
        ui.execute_action(UiAction::SetVizFpsCap(FpsCap::VSync));
        ui.is_window_focused = false;
        assert_eq!(
            ui.effective_frame_pacing().1,
            72,
            "Unfocused Visualizer VSync on 144Hz display should throttle cleanly to 72Hz"
        );
        ui.is_window_focused = true;
        assert_eq!(ui.effective_frame_pacing().1, 144);

        // 5. P0-5: active_queue_tracks_cached returns O(1) Arc clone + effective_adam_id_str zero-alloc + Lyrics idle lift skip
        let q1 = ui.active_queue_tracks_cached();
        let q2 = ui.active_queue_tracks_cached();
        assert!(
            Arc::ptr_eq(&q1, &q2),
            "Repeated active_queue_tracks_cached() calls without queue changes must reuse the same Arc slice"
        );
        let tr = AgentTrackInput {
            id: "i.abc".to_string(),
            catalog_id: Some("12345678".to_string()),
            ..Default::default()
        };
        assert_eq!(tr.effective_adam_id_str(), "12345678");
        assert_eq!(tr.effective_adam_id_str(), tr.effective_adam_id());

        let yrc_multi = "[1000,1000](1000,500,0)一(1500,500,0)二\n[5000,1000](5000,500,0)三(5500,500,0)四\n";
        let plyr = parse_yrc_lyrics(yrc_multi, None).expect("parse yrc");
        let mut anim = LyricsAnimator::default();
        anim.bind_lyrics("perf:skip", &plyr);
        anim.step(&plyr, 1.2, 600.0, 0.016);
        assert!(anim.line_states[0].has_active_lifts, "active line steps lifts");
        assert!(!anim.line_states[1].has_active_lifts, "inactive future line skips lift stepping");

        // 6. P2: i18n fast-reject preserves translations and avoids scanning non-UI CJK song metadata
        let media_title = translate_ui_text_for("青花瓷", ResolvedLang::En);
        assert!(matches!(media_title, Cow::Borrowed("青花瓷")));
        assert_eq!(
            translate_ui_text_for("待播清单 (12)", ResolvedLang::En).as_ref(),
            "Up Next (12)"
        );
        assert_eq!(
            translate_ui_text_for("Hi-Res 无损 · 杜比全景声", ResolvedLang::En).as_ref(),
            "Hi-Res Lossless · Dolby Atmos"
        );

        // 7. P2: VisualizerState::step_active steps ONLY active style (skips reactor pillars on AudioInspector!)
        let mut viz_active = am_light_lib::visualizers::VisualizerState::default();
        viz_active.style = am_light_lib::visualizers::VizStyle::AudioInspector;
        let mut loud_snap = am_light_lib::audio_viz::LiveVisualizerSnapshot {
            active_signal: true,
            rms_db: -18.0,
            peak_db_l: -12.0,
            peak_db_r: -12.0,
            bass_energy: 0.4,
            mid_energy: 0.4,
            high_energy: 0.2,
            ..Default::default()
        };
        loud_snap.spectrum_64.fill(0.35);
        for _ in 0..10 {
            viz_active.step_active(&loud_snap, 1.0 / 60.0, false);
        }
        assert_eq!(
            viz_active.matrix_pillar_stats().1,
            0.0,
            "step_active on AudioInspector must NOT burn CPU stepping inactive 37x37 Matrix Core"
        );
        viz_active.style = am_light_lib::visualizers::VizStyle::ChernobylRods;
        viz_active.reactor_sub_style = 0;
        for _ in 0..15 {
            viz_active.step_active(&loud_snap, 1.0 / 60.0, false);
        }
        assert!(
            viz_active.matrix_pillar_stats().1 > 8.0,
            "step_active on ChernobylRods Matrix Core must actively elevate pillars"
        );
    }

    #[test]
    fn test_m1_liquid_glass_pipeline_optimization() {
        use am_light_lib::am_api::{EditorialItem, EditorialShelf, EditorialShelfKind};
        use am_light_lib::app_ui::{AppUi, PageRoute};
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::d2d_renderer::D2DRenderer;
        use am_light_lib::lyrics::parse_lrc_lyrics;
        use am_light_lib::smtc::SmtcState;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        unsafe extern "system" fn m1_wnd_proc(
            h: HWND,
            m: u32,
            w: WPARAM,
            l: LPARAM,
        ) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }

        let hwnd = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = w!("AMLightM1PerfUnitHost");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(m1_wnd_proc),
                hInstance: hinst,
                lpszClassName: cls,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                cls,
                PCWSTR(w!("AM Light M1 Unit").as_ptr()),
                WS_POPUP,
                0,
                0,
                1400,
                880,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("CreateWindowExW")
        };

        let mut renderer = D2DRenderer::new(hwnd, 1400, 880, 1.0).expect("D2DRenderer::new 100%");
        let mut ui = AppUi::new(0);
        // Mutate settings directly (never execute_action) so %APPDATA%\AMLight\settings_v2.json is untouched.
        ui.settings.liquid_glass_enabled = true;
        ui.settings.reduce_transparency = false;
        ui.settings.reduce_motion = false;
        ui.settings.playback_backend = PlaybackBackend::NetEase;
        ui.settings.sidebar_collapsed = false;
        ui.route = PageRoute::Home;
        ui.show_lyrics_overlay = false;
        ui.route_trans = 1.0;
        ui.sidebar_open_spring.snap(1.0);
        ui.top_bar_guard_spring.snap(0.0);
        ui.mouse_pos = (-100.0, -100.0);
        ui.smtc = SmtcState {
            title: "Liquid Glass M1".to_string(),
            artist: "AM Light".to_string(),
            status: "Paused".to_string(),
            position_sec: 42.0,
            duration_sec: 210.0,
            ..Default::default()
        };
        ui.home_shelves = vec![
            EditorialShelf {
                title: "Top Picks".to_string(),
                subtitle: String::new(),
                kind: EditorialShelfKind::HomeTopPicks,
                items: (0..4)
                    .map(|i| EditorialItem {
                        id: format!("pick_{i}"),
                        kind: "album".to_string(),
                        play_kind: "album".to_string(),
                        title: format!("Featured Pick {i}"),
                        subtitle: "Editorial".to_string(),
                        ..Default::default()
                    })
                    .collect(),
            },
            EditorialShelf {
                title: "Recently Played".to_string(),
                subtitle: String::new(),
                kind: EditorialShelfKind::SquareShelf,
                items: (0..12)
                    .map(|i| EditorialItem {
                        id: format!("sq_{i}"),
                        kind: "album".to_string(),
                        play_kind: "album".to_string(),
                        title: format!("Album {i}"),
                        subtitle: "Artist".to_string(),
                        ..Default::default()
                    })
                    .collect(),
            },
        ];

        // 1. 100% DPI (1400x880, 1.0) steady-state Home
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        renderer.step_glass_animations(1.0);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        renderer.step_glass_animations(1.0);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_100 = renderer.frame_stats();
        assert_eq!(st_100.glass_push_layers, 0, "100% DPI must use ID2D1ImageBrush (0 PushLayers)");
        assert_eq!(st_100.glass_effect_rebinds, 0, "100% DPI steady-state must have 0 effect rebinds");
        assert_eq!(st_100.glass_shadow_geom_allocs, 0, "100% DPI steady-state must have 0 shadow geom allocs");
        assert!(
            st_100.glass_solid_fastpath_draws >= 6,
            "100% DPI top bar must take solid fastpath (>=6 draws), got {}",
            st_100.glass_solid_fastpath_draws
        );
        assert_eq!(st_100.glass_snapshot_flushes, 0, "100% DPI steady-state must have 0 snapshot flushes");
        assert_eq!(st_100.glass_snapshot_blurs, 0, "100% DPI steady-state must have 0 snapshot blurs");
        assert!(
            st_100.glass_snapshot_cache_hits >= 1,
            "100% DPI steady-state must reuse cached bottom snapshot strip"
        );

        let (rb_w, rb_h, bgra) = renderer.readback_backbuffer_bgra().expect("readback 100%");
        assert_eq!((rb_w, rb_h), (1400, 880));
        let has_non_bg_in_rect = |x0: u32, y0: u32, x1: u32, y1: u32| -> bool {
            for py in y0..y1.min(rb_h) {
                for px in x0..x1.min(rb_w) {
                    let i = ((py * rb_w + px) * 4) as usize;
                    let (b, g, r) = (bgra[i], bgra[i + 1], bgra[i + 2]);
                    if b.abs_diff(19) > 2 || g.abs_diff(16) > 2 || r.abs_diff(16) > 2 {
                        return true;
                    }
                }
            }
            false
        };
        assert!(
            has_non_bg_in_rect(1348, 14, 1382, 48),
            "top-right close glass button must render visible non-background pixels"
        );
        assert!(
            has_non_bg_in_rect(452, 804, 1212, 866),
            "bottom MiniPlayer glass pill must render visible non-background pixels"
        );

        // 2. Scrolled Home (scroll_y = 140.0) -> 1 flush + 1 blur on scroll, then 0 on settled frame
        ui.scroll_y = 140.0;
        ui.scroll_target_y = 140.0;
        ui.scroll_vel_y = 0.0;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_scroll = renderer.frame_stats();
        assert_eq!(st_scroll.glass_push_layers, 0, "scrolled Home must have 0 PushLayers");
        assert_eq!(st_scroll.glass_solid_fastpath_draws, 0, "scrolled Home must disable top-bar solid fastpath");
        assert_eq!(st_scroll.glass_snapshot_flushes, 1, "scrolled frame must issue exactly 1 pre-copy Flush");
        assert_eq!(st_scroll.glass_snapshot_blurs, 1, "scrolled frame must issue exactly 1 blur pass");

        // Warm pool slots for the 6 top-bar controls + MiniPlayer, then verify settled frame hits 0s
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_scroll_settled = renderer.frame_stats();
        assert_eq!(st_scroll_settled.glass_push_layers, 0);
        assert_eq!(st_scroll_settled.glass_effect_rebinds, 0, "settled scrolled Home must have 0 effect rebinds");
        assert_eq!(st_scroll_settled.glass_shadow_geom_allocs, 0);
        assert_eq!(st_scroll_settled.glass_snapshot_flushes, 0, "settled scrolled Home must have 0 flushes");
        assert_eq!(st_scroll_settled.glass_snapshot_blurs, 0, "settled scrolled Home must have 0 blurs");
        assert!(
            st_scroll_settled.glass_snapshot_cache_hits >= 1,
            "settled scrolled Home must reuse cached top & bottom strips"
        );

        // 3. Top-Bar Scrim (tb_guard_a > 0.5 at scroll_y = 0.0)
        ui.scroll_y = 0.0;
        ui.scroll_target_y = 0.0;
        ui.mouse_pos = (700.0, 20.0);
        ui.top_bar_guard_spring.snap(1.0);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_scrim = renderer.frame_stats();
        assert_eq!(st_scrim.glass_push_layers, 0);
        assert!(
            st_scrim.glass_solid_fastpath_draws >= 6,
            "scrim top bar at scroll_y=0 must still use solid fastpath (>=6), got {}",
            st_scrim.glass_solid_fastpath_draws
        );
        assert_eq!(st_scrim.glass_snapshot_blurs, 0, "scrim top bar settled frame must have 0 blurs");
        ui.mouse_pos = (-100.0, -100.0);
        ui.top_bar_guard_spring.snap(0.0);

        // 4. !use_batch_snapshot fallback parity (More popover rendered after end_glass_backdrop_snapshot)
        ui.show_more_popover = true;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_fallback = renderer.frame_stats();
        assert_eq!(st_fallback.glass_push_layers, 0, "fallback popover must also use ID2D1ImageBrush (0 PushLayers)");
        assert!(st_fallback.glass_snapshot_flushes >= 1, "fallback popover must perform local capture flush");
        ui.show_more_popover = false;
        // Confirm batch pool & cached snapshot remain intact after fallback
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_after_fb = renderer.frame_stats();
        assert_eq!(st_after_fb.glass_effect_rebinds, 0, "fallback must not corrupt batch pool slots");
        assert_eq!(st_after_fb.glass_snapshot_flushes, 0);
        assert_eq!(st_after_fb.glass_snapshot_blurs, 0);

        // 5. reduce_transparency = true
        ui.settings.reduce_transparency = true;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_red = renderer.frame_stats();
        assert_eq!(st_red.glass_push_layers, 0);
        assert_eq!(st_red.glass_snapshot_flushes, 0);
        assert_eq!(st_red.glass_snapshot_blurs, 0);
        assert_eq!(st_red.glass_solid_fastpath_draws, 0);
        ui.settings.reduce_transparency = false;

        // 6. 200% HiDPI (2800x1760, 2.0) + resize(2400, 1600, 2.0) invalidation + DPI change (1.0)
        renderer.resize(2800, 1760, 2.0);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        renderer.step_glass_animations(1.0);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_200 = renderer.frame_stats();
        assert_eq!(st_200.glass_push_layers, 0, "200% DPI must have 0 PushLayers");
        assert_eq!(st_200.glass_effect_rebinds, 0, "200% DPI steady-state must have 0 effect rebinds");
        assert_eq!(st_200.glass_shadow_geom_allocs, 0, "200% DPI steady-state must have 0 shadow allocs");
        assert!(st_200.glass_solid_fastpath_draws >= 6);
        assert_eq!(st_200.glass_snapshot_flushes, 0);
        assert_eq!(st_200.glass_snapshot_blurs, 0);
        assert!(st_200.glass_snapshot_cache_hits >= 1);

        let (rb_w2, rb_h2, bgra2) = renderer.readback_backbuffer_bgra().expect("readback 200%");
        assert_eq!((rb_w2, rb_h2), (2800, 1760));
        let has_2x_close = (28..96).any(|py| {
            (2696..2764).any(|px| {
                let i = ((py * rb_w2 + px) * 4) as usize;
                bgra2[i].abs_diff(19) > 2 || bgra2[i + 1].abs_diff(16) > 2 || bgra2[i + 2].abs_diff(16) > 2
            })
        });
        assert!(has_2x_close, "200% DPI top-right close button must render visible non-background pixels");

        renderer.resize(2400, 1600, 2.0);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_resz = renderer.frame_stats();
        assert_eq!(st_resz.glass_push_layers, 0);
        assert_eq!(st_resz.glass_snapshot_flushes, 1, "resize must invalidate snapshot cache");
        assert_eq!(st_resz.glass_snapshot_blurs, 1);
        assert_eq!(st_resz.glass_snapshot_cache_hits, 0);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_resz2 = renderer.frame_stats();
        assert_eq!(st_resz2.glass_effect_rebinds, 0);
        assert_eq!(st_resz2.glass_snapshot_flushes, 0);
        assert_eq!(st_resz2.glass_snapshot_blurs, 0);

        renderer.resize(1200, 800, 1.0);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_dpi_drop = renderer.frame_stats();
        assert_eq!(st_dpi_drop.glass_push_layers, 0);
        assert_eq!(st_dpi_drop.glass_snapshot_flushes, 1);

        // 7. LyricsViewState wrapper parity (`begin_glass_backdrop_snapshot_strips`)
        ui.current_lyrics = parse_lrc_lyrics("[00:01.00]Line One\n[00:05.00]Line Two\n", None, "test");
        ui.current_lyrics_key = "m1_lyrics_test".to_string();
        ui.show_lyrics_overlay = true;
        ui.lyrics_open_spring.snap(1.0);
        ui.mouse_pos = (60.0, 30.0);
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let st_lyr = renderer.frame_stats();
        assert_eq!(st_lyr.glass_push_layers, 0, "LyricsViewState must have 0 PushLayers");
        assert_eq!(st_lyr.glass_snapshot_flushes, 1, "LyricsViewState strip wrapper must issue 1 flush");
        assert_eq!(st_lyr.glass_snapshot_blurs, 1, "LyricsViewState strip wrapper must issue 1 blur");

        // 8. Iteration 2 Regressions:
        //    8a. Single-frame readback_backbuffer_bgra() after Present() returns the exact just-presented frame
        {
            use am_light_lib::theme::{Color, RectF};
            renderer.resize(880, 880, 1.0);
            renderer.begin_draw(Color::rgb(21, 99, 177));
            renderer.fill_rect(RectF::new(100.0, 100.0, 120.0, 120.0), Color::rgb(220, 45, 55));
            renderer.end_window_draw_and_present(0.0, true, 0);
            let (rw, rh, rbuf) = renderer.readback_backbuffer_bgra().expect("single-frame readback");
            assert_eq!((rw, rh), (880, 880));
            let bg_i = ((50 * rw + 50) * 4) as usize;
            let fg_i = ((150 * rw + 150) * 4) as usize;
            assert!(
                rbuf[bg_i].abs_diff(177) <= 2 && rbuf[bg_i + 1].abs_diff(99) <= 2 && rbuf[bg_i + 2].abs_diff(21) <= 2,
                "single-frame readback bg mismatch: {:?}",
                &rbuf[bg_i..bg_i + 4]
            );
            assert!(
                rbuf[fg_i].abs_diff(55) <= 2 && rbuf[fg_i + 1].abs_diff(45) <= 2 && rbuf[fg_i + 2].abs_diff(220) <= 2,
                "single-frame readback fg mismatch: {:?}",
                &rbuf[fg_i..fg_i + 4]
            );
        }

        //    8b. LibSongs playback_loading spinner_phase (0.0 -> 0.5) refreshes snapshot & updates MiniPlayer pixels
        {
            use am_light_lib::am_agent::AgentTrackInput;
            use am_light_lib::app_ui::ContainerDetailData;
            use am_light_lib::downloads::{DownloadedTrackItem, DownloadsOverview};

            ui.show_lyrics_overlay = false;
            ui.lyrics_open_spring.snap(0.0);
            ui.mouse_pos = (-100.0, -100.0);
            ui.top_bar_guard_spring.snap(0.0);
            ui.sidebar_open_spring.snap(1.0);
            ui.settings.sidebar_collapsed = false;
            ui.settings.reduce_motion = false;
            ui.scroll_y = 0.0;
            ui.scroll_target_y = 0.0;
            ui.scroll_vel_y = 0.0;
            renderer.resize(880, 880, 1.0);

            let make_tracks = |prefix: &str| -> Vec<AgentTrackInput> {
                (0..25)
                    .map(|i| AgentTrackInput {
                        id: format!("trk_{i}"),
                        catalog_id: Some(format!("{}", 10000 + i)),
                        kind: Some("songs".to_string()),
                        name: format!("{prefix} {i}"),
                        artist_name: format!("Artist {i}"),
                        album_name: "Album".to_string(),
                        duration_sec: 180.0,
                        artwork_url: None,
                        audio_traits: Vec::new(),
                    })
                    .collect()
            };

            ui.route = PageRoute::LibSongs;
            ui.lib_songs = make_tracks("Song");
            ui.playback_loading = true;
            ui.playback_loading_track_id = Some("10012".to_string());
            ui.spinner_phase = 0.0;
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            let (_, _, spin_px_a) = renderer.readback_backbuffer_bgra().expect("spin A");

            ui.spinner_phase = 0.5;
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            let st_spin = renderer.frame_stats();
            assert!(
                st_spin.glass_snapshot_blurs >= 1,
                "advancing spinner_phase during playback_loading must refresh snapshot"
            );
            let (rw, rh, spin_px_b) = renderer.readback_backbuffer_bgra().expect("spin B");
            let mini_max_diff = (804..866.min(rh as usize))
                .flat_map(|py| (290..856.min(rw as usize)).map(move |px| (py, px)))
                .map(|(py, px)| {
                    let i = (py * rw as usize + px) * 4;
                    spin_px_a[i]
                        .abs_diff(spin_px_b[i])
                        .max(spin_px_a[i + 1].abs_diff(spin_px_b[i + 1]))
                        .max(spin_px_a[i + 2].abs_diff(spin_px_b[i + 2]))
                })
                .max()
                .unwrap_or(0);
            assert!(
                mini_max_diff >= 3,
                "spinner_phase change behind MiniPlayer must visually update pixels (max_diff={mini_max_diff})"
            );

            //    8c. Same-.len() content mutations on LibSongs, ContainerDetail, LibDownloads
            ui.playback_loading = false;
            ui.playback_loading_track_id = None;
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert_eq!(renderer.frame_stats().glass_snapshot_blurs, 0);

            for i in 10..16 {
                ui.lib_songs[i].name = format!("Mutated Lib Title {i} WWWWW");
            }
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert!(
                renderer.frame_stats().glass_snapshot_blurs >= 1,
                "same-.len() LibSongs mutation must trigger snapshot refresh"
            );

            ui.route = PageRoute::ContainerDetail {
                id: "det_1".to_string(),
                kind: "album".to_string(),
                title_hint: "Album".to_string(),
            };
            ui.detail_loading = false;
            ui.detail_data = Some(ContainerDetailData {
                id: "det_1".to_string(),
                kind: "album".to_string(),
                title: "Album Title".to_string(),
                subtitle: "Artist".to_string(),
                description: String::new(),
                artwork_url: None,
                motion_video_url: None,
                tracks: make_tracks("DetailTrack"),
                related_albums: Vec::new(),
            });
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert_eq!(renderer.frame_stats().glass_snapshot_blurs, 0);
            if let Some(ref mut d) = ui.detail_data {
                for i in 6..15 {
                    d.tracks[i].name = format!("Mutated Detail {i} WWWWW");
                }
            }
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert!(
                renderer.frame_stats().glass_snapshot_blurs >= 1,
                "same-.len() ContainerDetail tracks mutation must trigger snapshot refresh"
            );

            ui.route = PageRoute::LibDownloads;
            ui.downloads_info = DownloadsOverview {
                media_folder: "C:\\Media".to_string(),
                folder_exists: true,
                total_bytes: 1024 * 1024 * 20,
                complete_count: 20,
                partial_count: 0,
                items: (0..20)
                    .map(|i| DownloadedTrackItem {
                        path: format!("C:\\Media\\t_{i}.movpkg"),
                        title: format!("DL {i}"),
                        track_number: Some(i as u32 + 1),
                        artist: "Artist".to_string(),
                        album: "Album".to_string(),
                        format: "ALAC".to_string(),
                        resolution: None,
                        bitrate_kbps: None,
                        size_bytes: 1024 * 1024,
                        complete: true,
                        modified_unix: 100,
                    })
                    .collect(),
            };
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert_eq!(renderer.frame_stats().glass_snapshot_blurs, 0);
            for i in 10..18 {
                ui.downloads_info.items[i].title = format!("Mutated DL {i} WWWWW");
            }
            ui.downloads_info.total_bytes += 512;
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert!(
                renderer.frame_stats().glass_snapshot_blurs >= 1,
                "same-.len() LibDownloads mutation must trigger snapshot refresh"
            );

            //    8d. PageRoute::Search live search_query edit triggers snapshot refresh
            ui.route = PageRoute::Search;
            ui.search_filter = 1;
            ui.scroll_y = 25.0;
            ui.scroll_target_y = 25.0;
            ui.search_query = "A".to_string();
            ui.search_completed_query = "A".to_string();
            ui.search_songs = make_tracks("SearchHit");
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert_eq!(renderer.frame_stats().glass_snapshot_blurs, 0);
            ui.search_query = "WWWWWWWWWWWWWWWWWWWWWWWWWWWW".to_string();
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert!(
                renderer.frame_stats().glass_snapshot_blurs >= 1,
                "live search_query edit on PageRoute::Search must trigger snapshot refresh"
            );

            //    8e. Home partially-occluded motion card (0 < cy + card_h <= 46.0) & unfocused tracking
            ui.route = PageRoute::Home;
            ui.settings.playback_backend = PlaybackBackend::AppleMusicNative;
            ui.settings.motion_artwork_enabled = true;
            ui.is_window_focused = false;
            ui.home_shelves = vec![
                EditorialShelf {
                    title: "Top Picks".to_string(),
                    subtitle: String::new(),
                    kind: EditorialShelfKind::HomeTopPicks,
                    items: vec![EditorialItem {
                        id: "peek_1".to_string(),
                        kind: "album".to_string(),
                        play_kind: "album".to_string(),
                        title: "Peek".to_string(),
                        subtitle: "Sub".to_string(),
                        motion_video_url: Some("https://example.invalid/top_peek.m3u8".to_string()),
                        ..Default::default()
                    }],
                },
                EditorialShelf {
                    title: "Below".to_string(),
                    subtitle: String::new(),
                    kind: EditorialShelfKind::SquareShelf,
                    items: (0..12)
                        .map(|i| EditorialItem {
                            id: format!("below_{i}"),
                            kind: "album".to_string(),
                            play_kind: "album".to_string(),
                            title: format!("Below {i}"),
                            subtitle: "Sub".to_string(),
                            ..Default::default()
                        })
                        .collect(),
                },
            ];
            // At win_w=880, sb=264 -> inner_w = 616-64 = 552, cols=2, card_w=267, card_h=341.76
            // start_y = 62 - scroll_y; header+title = 86; card y = start_y + 86 = 148 - scroll_y
            // card bottom = 148 + 341.76 - scroll_y -> set scroll_y = 460.0 -> cy+card_h ~= 29.8 in (0, 46]!
            ui.scroll_y = 460.0;
            ui.scroll_target_y = 460.0;
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert!(
                ui.rendered_home_motion_urls()
                    .iter()
                    .any(|u| u == "https://example.invalid/top_peek.m3u8"),
                "partially-occluded top-bar Home motion card (0 < cy+card_h <= 46) must be tracked even when unfocused"
            );
            ui.is_window_focused = true;
            ui.settings.playback_backend = PlaybackBackend::NetEase;

            //    8f. top_bar_solid_bg grip occlusion at win_w = 880, tb_guard_a = 1.0, in_room = true
            ui.scroll_y = 0.0;
            ui.scroll_target_y = 0.0;
            ui.top_bar_guard_spring.snap(1.0);
            ui.listen_together.in_room = true;
            ui.listen_together.room_id = "room1".to_string();
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert_eq!(
                renderer.frame_stats().glass_solid_fastpath_draws, 0,
                "win_w=880 with tb_guard_a=1.0 and overlapping Listen Together must disable top_bar_solid_bg"
            );
            let (gw, gh, grip_at_0) = renderer.readback_backbuffer_bgra().expect("grip scroll=0");
            ui.scroll_y = 2.5;
            ui.scroll_target_y = 2.5;
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            let (_, _, grip_at_25) = renderer.readback_backbuffer_bgra().expect("grip scroll=2.5");
            let grip_max_diff = (24..34.min(gh as usize))
                .flat_map(|py| (564..604.min(gw as usize)).map(move |px| (py, px)))
                .map(|(py, px)| {
                    let i = (py * gw as usize + px) * 4;
                    grip_at_0[i]
                        .abs_diff(grip_at_25[i])
                        .max(grip_at_0[i + 1].abs_diff(grip_at_25[i + 1]))
                        .max(grip_at_0[i + 2].abs_diff(grip_at_25[i + 2]))
                })
                .max()
                .unwrap_or(0);
            assert!(
                grip_max_diff <= 4,
                "lt_r over grip at win_w=880 must match GPU blur within <= 4 (got {grip_max_diff})"
            );

            // Confirm wide win_w=1400 still uses fastpath when grip does NOT overlap lt_r
            renderer.resize(1400, 880, 1.0);
            ui.scroll_y = 0.0;
            ui.scroll_target_y = 0.0;
            ui.step_and_render_ex(&mut renderer, 0.0, false);
            assert!(
                renderer.frame_stats().glass_solid_fastpath_draws >= 6,
                "win_w=1400 non-overlapping grip must keep top_bar_solid_bg fastpath"
            );
            ui.listen_together.in_room = false;
            ui.top_bar_guard_spring.snap(0.0);
        }

        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn test_r1_home_motion_expanded_slots_prefetch_warm_lru_crossfade_and_sizing() {
        use am_light_lib::am_api::{EditorialItem, EditorialShelf, EditorialShelfKind};
        use am_light_lib::app_ui::{AppUi, PageRoute};
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::d2d_renderer::D2DRenderer;
        use am_light_lib::motion_art::{
            MotionArtManager, MAX_VIEWPORT_SLOTS, MAX_WARM_PAUSED_SLOTS,
        };
        use am_light_lib::smtc::SmtcState;
        use std::time::Duration;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        assert_eq!(MAX_VIEWPORT_SLOTS, 20);
        assert_eq!(MAX_WARM_PAUSED_SLOTS, 28);

        // 1. Direct MotionArtManager R1 checks: 20 active slots, bounded 28-entry warm-paused LRU,
        //    warm-cache hit counter on re-entry, and scroll-active prefetch telemetry.
        {
            let mgr = MotionArtManager::new();
            let batch24: Vec<(String, u32)> = (0..24)
                .map(|i| (format!("https://example.invalid/r1_slot_{i}.m3u8"), 220 + (i as u32) * 10))
                .collect();
            mgr.sync_visible_urls_with_targets(&batch24);
            assert_eq!(
                mgr.active_slot_urls().len(),
                20,
                "expanded active viewport budget must hold MAX_VIEWPORT_SLOTS=20 (>=18)"
            );
            assert_eq!(mgr.telemetry_snapshot().active_slots, 20);

            // Demote first 20 slots -> warm LRU holds 20; activate & demote 16 more -> bounded at 28
            let batch16: Vec<(String, u32)> = (24..40)
                .map(|i| (format!("https://example.invalid/r1_slot_{i}.m3u8"), 300))
                .collect();
            mgr.sync_visible_urls_with_targets(&batch16);
            assert_eq!(mgr.telemetry_snapshot().warm_slots, 20);
            mgr.sync_visible_urls_with_targets(&[]);
            assert_eq!(
                mgr.telemetry_snapshot().warm_slots,
                MAX_WARM_PAUSED_SLOTS,
                "warm-paused LRU must stay strictly bounded at 28 entries"
            );

            // Re-entering a warm-paused URL promotes it and increments warm_cache_hits
            let hits_before = mgr.telemetry_snapshot().warm_cache_hits;
            mgr.sync_visible_urls_with_targets(&[(
                "https://example.invalid/r1_slot_35.m3u8".to_string(),
                300,
            )]);
            let tel_after = mgr.telemetry_snapshot();
            assert!(
                tel_after.warm_cache_hits > hits_before,
                "re-entering a warm-paused slot must bump warm_cache_hits"
            );
            assert_eq!(tel_after.active_slots, 1);

            // Repeated sync_prefetch_urls must queue new URLs once without double-counting
            let pf_list: Vec<(String, u32)> = (100..108)
                .map(|i| (format!("https://example.invalid/r1_pf_{i}.m3u8"), 260))
                .collect();
            mgr.sync_prefetch_urls(&pf_list);
            let q1 = mgr.telemetry_snapshot().prefetch_queued_during_scroll;
            assert_eq!(q1, 8);
            mgr.sync_prefetch_urls(&pf_list);
            assert_eq!(
                mgr.telemetry_snapshot().prefetch_queued_during_scroll,
                q1,
                "duplicate prefetch calls must not double-count or spam worker"
            );
            mgr.set_global_enabled(false);
        }

        // 2. Real D2DRenderer + AppUi Home R1 test: larger showcase sizing, >=18 visible slots,
        //    top-bar overlap inclusion, scroll-active prefetch, and instant cached-MP4 activation!
        unsafe extern "system" fn r1_wnd_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }
        let hwnd = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = w!("AMLightR1UnitHost");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(r1_wnd_proc),
                hInstance: hinst,
                lpszClassName: cls,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                cls,
                PCWSTR(w!("AM Light R1 Unit").as_ptr()),
                WS_POPUP,
                0,
                0,
                1400,
                1000,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("CreateWindowExW R1")
        };

        let mut renderer = D2DRenderer::new(hwnd, 1400, 1000, 1.0).expect("D2DRenderer::new R1");
        let mut ui = AppUi::new(0);
        ui.settings.playback_backend = PlaybackBackend::AppleMusicNative;
        ui.settings.motion_artwork_enabled = true;
        ui.settings.reduce_motion = false;
        ui.settings.liquid_glass_enabled = true;
        ui.settings.reduce_transparency = false;
        ui.settings.sidebar_collapsed = false;
        ui.is_window_focused = true;
        ui.route = PageRoute::Home;
        ui.show_lyrics_overlay = false;
        ui.route_trans = 1.0;
        ui.sidebar_open_spring.snap(1.0);
        ui.top_bar_guard_spring.snap(0.0);
        ui.mouse_pos = (-100.0, -100.0);
        ui.smtc = SmtcState {
            title: "R1 Home Motion".to_string(),
            artist: "AM Light".to_string(),
            status: "Paused".to_string(),
            position_sec: 12.0,
            duration_sec: 200.0,
            ..Default::default()
        };

        let make_item = |id: &str, url: &str, tall: bool| EditorialItem {
            id: id.to_string(),
            kind: "album".to_string(),
            play_kind: "album".to_string(),
            title: format!("Item {id}"),
            subtitle: "Editorial".to_string(),
            motion_video_url: if tall { None } else { Some(url.to_string()) },
            motion_tall_video_url: if tall { Some(url.to_string()) } else { None },
            ..Default::default()
        };

        // 2a. Verify spacious desktop sizing for HomeTopPicks, HeroFlowcase & SquareShelf (1400x1000)
        ui.home_shelves = vec![
            EditorialShelf {
                title: "Top Picks".to_string(),
                subtitle: String::new(),
                kind: EditorialShelfKind::HomeTopPicks,
                items: (0..6)
                    .map(|i| make_item(&format!("tp_{i}"), &format!("https://example.invalid/tp_{i}.m3u8"), true))
                    .collect(),
            },
            EditorialShelf {
                title: "Featured".to_string(),
                subtitle: String::new(),
                kind: EditorialShelfKind::HeroFlowcase,
                items: (0..6)
                    .map(|i| make_item(&format!("hf_{i}"), &format!("https://example.invalid/hf_{i}.m3u8"), false))
                    .collect(),
            },
            EditorialShelf {
                title: "Recently Played".to_string(),
                subtitle: String::new(),
                kind: EditorialShelfKind::SquareShelf,
                items: (0..12)
                    .map(|i| make_item(&format!("sq_{i}"), &format!("https://example.invalid/sq_{i}.m3u8"), false))
                    .collect(),
            },
        ];
        std::thread::sleep(Duration::from_millis(95));
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);

        let tp_card = ui
            .rendered_home_motion_cards()
            .iter()
            .find(|(u, _)| u.ends_with("/tp_0.m3u8"))
            .map(|(_, r)| *r)
            .expect("Top Picks card 0 rendered");
        assert!(
            tp_card.w >= 260.0 && tp_card.h >= 245.0 && tp_card.h <= 415.5,
            "Desktop HomeTopPicks must preserve classic Apple Music sizing (<=415 DIP height), got {:?}",
            tp_card
        );

        // Scroll to inspect HeroFlowcase + SquareShelf classic desktop card geometry
        ui.scroll_y = 460.0;
        ui.scroll_target_y = 460.0;
        ui.scroll_vel_y = 0.0;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let hf_card = ui
            .rendered_home_motion_cards()
            .iter()
            .find(|(u, _)| u.ends_with("/hf_0.m3u8"))
            .map(|(_, r)| *r)
            .expect("HeroFlowcase card 0 rendered");
        assert!(
            hf_card.w >= 320.0 && hf_card.h >= 180.0 && hf_card.h <= 290.5,
            "Desktop HeroFlowcase must preserve classic sizing (180..=290 DIP height), got {:?}",
            hf_card
        );

        ui.scroll_y = 1040.0;
        ui.scroll_target_y = 1040.0;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        let sq_card = ui
            .rendered_home_motion_cards()
            .iter()
            .find(|(u, _)| u.ends_with("/sq_0.m3u8"))
            .map(|(_, r)| *r)
            .expect("SquareShelf card 0 rendered");
        assert!(
            sq_card.w >= 160.0 && sq_card.w <= 220.0 && (sq_card.w - sq_card.h).abs() < 1.0,
            "Desktop Home SquareShelf motion card must preserve classic 6-col square sizing, got {:?}",
            sq_card
        );

        // Verify narrow viewport does not overflow content area
        renderer.resize(780, 760, 1.0);
        ui.scroll_y = 0.0;
        ui.scroll_target_y = 0.0;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        for (_, cr) in ui.rendered_home_motion_cards() {
            assert!(
                cr.right() <= 780.5 && cr.x >= -0.5,
                "Narrow Home card must not overflow viewport: {:?}",
                cr
            );
        }

        // 2b. Expanded active viewport slots (>= 18, up to 20) + top-bar overlap inclusion + offscreen prefetch
        renderer.resize(1760, 1360, 1.0);
        ui.home_shelves = (0..6)
            .map(|s| EditorialShelf {
                title: format!("Shelf {s}"),
                subtitle: String::new(),
                kind: EditorialShelfKind::SquareShelf,
                items: (0..10)
                    .map(|i| {
                        make_item(
                            &format!("m_{s}_{i}"),
                            &format!("https://example.invalid/m_{s}_{i}.m3u8"),
                            false,
                        )
                    })
                    .collect(),
            })
            .collect();
        ui.scroll_y = 0.0;
        ui.scroll_target_y = 0.0;
        ui.scroll_vel_y = 0.0;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        assert!(
            ui.visible_home_motion_urls().len() >= 18
                && ui.visible_home_motion_urls().len() <= MAX_VIEWPORT_SLOTS,
            "Expanded Home viewport must track >=18 (up to 20) active slots, got {}",
            ui.visible_home_motion_urls().len()
        );
        assert_eq!(
            ui.active_motion_slot_urls().len(),
            ui.visible_home_motion_urls().len(),
            "Settled Home must activate all visible viewport motion slots"
        );
        assert!(
            !ui.prefetch_home_motion_targets().is_empty(),
            "Offscreen & horizontal-lookahead cards must populate prefetch_home_motion_targets"
        );
        assert!(
            ui.motion_telemetry().prefetch_queued_during_scroll > 0,
            "Prefetch queue telemetry must increment"
        );

        // Top-bar overlap inclusion: scroll shelf 0 so its card bottom sits strictly in (0, 46] DIP
        // (use a standard 700 DIP tall viewport so 3 rows = 18 cards <= 20-slot cap while keeping 6 shelves scrollable)
        renderer.resize(1760, 700, 1.0);
        let first_card_h = ui
            .rendered_home_motion_cards()
            .first()
            .map(|(_, r)| r.h)
            .unwrap_or(240.0);
        let first_card_y0 = ui
            .rendered_home_motion_cards()
            .first()
            .map(|(_, r)| r.y)
            .unwrap_or(148.0);
        let peek_scroll = (first_card_y0 + first_card_h) - 24.0;
        ui.scroll_y = peek_scroll;
        ui.scroll_target_y = peek_scroll;
        ui.scroll_vel_y = 0.0;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
        assert!(
            ui.visible_home_motion_urls()
                .iter()
                .any(|u| u.ends_with("/m_0_0.m3u8")),
            "Card peeking strictly underneath floating top bar (0 < bottom <= 46) must stay in active viewport slots"
        );
        renderer.resize(1760, 1360, 1.0);

        // 2c. Instant cached-MP4 activation (0ms settle bypass) + scroll-active prefetch during FAST scroll!
        let cached_url = format!("https://example.invalid/r1_cached_fast_{}.m3u8", std::process::id());
        let uncached_url = format!("https://example.invalid/r1_uncached_fast_{}.m3u8", std::process::id());
        let slot_k = MotionArtManager::slot_key_for_url(&cached_url);
        let hash_hex = slot_k.strip_prefix("u_").expect("u_ prefix");
        let cache_dir = std::path::PathBuf::from(
            std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"),
        )
        .join("AM-Light")
        .join("cache")
        .join("motion_art");
        let _ = std::fs::create_dir_all(&cache_dir);
        let fake_mp4 = cache_dir.join(format!("{hash_hex}.mp4"));
        std::fs::write(&fake_mp4, vec![0xAAu8; 16_384]).expect("write temp cached mp4");
        assert!(
            ui.motion_art_manager().is_url_cached_ready(&cached_url),
            "Existing >8KB cached MP4 must report is_url_cached_ready == true"
        );
        assert!(
            !ui.motion_art_manager().is_url_cached_ready(&uncached_url),
            "Missing MP4 must report is_url_cached_ready == false"
        );

        // Put cached_url and uncached_url on screen + offscreen prefetch card, then fast-scroll!
        let offscreen_pf_url =
            format!("https://example.invalid/r1_scroll_pf_{}.m3u8", std::process::id());
        ui.home_shelves = vec![
            EditorialShelf {
                title: "Fast Scroll".to_string(),
                subtitle: String::new(),
                kind: EditorialShelfKind::HomeTopPicks,
                items: vec![
                    make_item("fast_cached", &cached_url, true),
                    make_item("fast_uncached", &uncached_url, true),
                    make_item("fast_extra1", "https://example.invalid/r1_fast_extra1.m3u8", true),
                    make_item("fast_extra2", "https://example.invalid/r1_fast_extra2.m3u8", true),
                    make_item("fast_lookahead", &offscreen_pf_url, true),
                ],
            },
        ];
        ui.scroll_y = 0.0;
        ui.scroll_target_y = 260.0;
        ui.scroll_vel_y = 820.0; // actively fast-scrolling!
        let pf_before_scroll = ui.motion_telemetry().prefetch_queued_during_scroll;
        ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);

        let fast_active = ui.active_motion_slot_urls();
        assert!(
            fast_active.iter().any(|u| u == &cached_url),
            "Cached MP4 must bypass scroll-settle debounce and activate immediately during fast scroll!"
        );
        assert!(
            !fast_active.iter().any(|u| u == &uncached_url),
            "Uncached network HLS card must wait out brief settle debounce while prefetching"
        );
        assert!(
            ui.motion_telemetry().prefetch_queued_during_scroll > pf_before_scroll,
            "Scroll-active prefetch must keep queueing offscreen/uncached URLs during fast scroll"
        );
        // Verify crossfade alpha starts at 0.0 before a decoded frame arrives (static cover only, zero black flash)
        assert_eq!(ui.motion_crossfade_alpha(&uncached_url), 0.0);

        // Clean up temporary synthetic cache file and stop worker cleanly
        ui.motion_art_manager().set_global_enabled(false);
        drop(ui);
        let _ = std::fs::remove_file(&fake_mp4);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn test_r3_dxva_target_dim_hls_parser_downscaler_and_gpu_fallback() {
        use am_light_lib::app_ui::AppUi;
        use am_light_lib::d2d_renderer::{D2DRenderer, MOTION_TEXTURE_LRU_CAP};
        use am_light_lib::motion_art::{
            bucket_target_dim, compute_scaled_dimensions, downscale_bgra_frame,
            parse_avc_variant_for_target_dim, parse_best_avc_variant_from_master,
            MotionFrameSnapshot,
        };
        use am_light_lib::theme::{Color, RectF};
        use std::sync::Arc;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::Graphics::Direct3D11::{
            ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
            D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
        };
        use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        // 1. parse_avc_variant_for_target_dim picks smallest AVC >= target_px (ignores HEVC & trickplay)
        let master_url = "https://mvod.itunes.apple.com/motion/master.m3u8";
        let m3u8 = r#"#EXTM3U
#EXT-X-VERSION:7
#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=60000,RESOLUTION=256x256,CODECS="avc1.64001f",URI="iframe_256.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=2600000,RESOLUTION=384x384,CODECS="hvc1.1.6.L90.90"
hevc_384.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=240x240,CODECS="avc1.640015"
avc_240.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=650000,RESOLUTION=360x360,CODECS="avc1.64001e"
avc_360.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=950000,RESOLUTION=486x486,CODECS="avc1.64001f"
avc_486.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=1800000,RESOLUTION=720x720,CODECS="avc1.64001f"
avc_720.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=3400000,RESOLUTION=1080x1080,CODECS="avc1.640028"
avc_1080.m3u8
"#;
        assert_eq!(
            parse_avc_variant_for_target_dim(master_url, m3u8, 256).as_deref(),
            Some("https://mvod.itunes.apple.com/motion/avc_360.m3u8")
        );
        assert_eq!(
            parse_avc_variant_for_target_dim(master_url, m3u8, 360).as_deref(),
            Some("https://mvod.itunes.apple.com/motion/avc_360.m3u8")
        );
        assert_eq!(
            parse_avc_variant_for_target_dim(master_url, m3u8, 384).as_deref(),
            Some("https://mvod.itunes.apple.com/motion/avc_486.m3u8")
        );
        assert_eq!(
            parse_avc_variant_for_target_dim(master_url, m3u8, 512).as_deref(),
            Some("https://mvod.itunes.apple.com/motion/avc_720.m3u8")
        );
        assert_eq!(
            parse_avc_variant_for_target_dim(master_url, m3u8, 2000).as_deref(),
            Some("https://mvod.itunes.apple.com/motion/avc_1080.m3u8")
        );
        assert_eq!(
            parse_avc_variant_for_target_dim(master_url, m3u8, 0),
            parse_best_avc_variant_from_master(m3u8, master_url)
        );

        // 2. Bucket & aspect-preserving downscale math + box/bilinear CPU resampler
        assert_eq!(bucket_target_dim(0), 384);
        assert_eq!(bucket_target_dim(180), 256);
        assert_eq!(bucket_target_dim(256), 256);
        assert_eq!(bucket_target_dim(280), 320);
        assert_eq!(bucket_target_dim(360), 384);
        assert_eq!(bucket_target_dim(480), 512);
        assert_eq!(compute_scaled_dimensions(486, 486, 256), (256, 256));
        assert_eq!(compute_scaled_dimensions(200, 200, 384), (200, 200));
        assert_eq!(compute_scaled_dimensions(1080, 1920, 384), (216, 384));

        // Box-area downscale (>=2x): 512x512 -> 256x256 solid color exactness
        let mut src_512 = vec![0u8; 512 * 512 * 4];
        for px in src_512.chunks_exact_mut(4) {
            px[0] = 200; // B
            px[1] = 120; // G
            px[2] = 40;  // R
            px[3] = 255;
        }
        let dst_256 = downscale_bgra_frame(&src_512, 512, 512, 256, 256);
        assert_eq!(dst_256.len(), 256 * 256 * 4);
        assert_eq!(&dst_256[0..4], &[200, 120, 40, 255]);
        assert_eq!(&dst_256[dst_256.len() - 4..], &[200, 120, 40, 255]);

        // Bilinear downscale (<2x): 486x486 -> 320x320 solid color exactness
        let mut src_486 = vec![0u8; 486 * 486 * 4];
        for px in src_486.chunks_exact_mut(4) {
            px[0] = 50;
            px[1] = 180;
            px[2] = 230;
            px[3] = 255;
        }
        let dst_320 = downscale_bgra_frame(&src_486, 486, 486, 320, 320);
        assert_eq!(dst_320.len(), 320 * 320 * 4);
        assert_eq!(&dst_320[0..4], &[50, 180, 230, 255]);

        // 3. Real D2DRenderer zero-copy DXVA GPU surface wrapping, CPU fallback byte tracking,
        //    opacity crossfade blending, and 28-entry LRU cap!
        unsafe extern "system" fn r3_wnd_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }
        let hwnd = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = w!("AMLightR3UnitHost");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(r3_wnd_proc),
                hInstance: hinst,
                lpszClassName: cls,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                cls,
                PCWSTR(w!("AM Light R3 Unit").as_ptr()),
                WS_POPUP,
                0,
                0,
                640,
                640,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("CreateWindowExW R3")
        };

        let mut renderer = D2DRenderer::new(hwnd, 640, 640, 1.0).expect("D2DRenderer::new R3");
        let d3d_dev = renderer.d3d_device().expect("D3D11Device must be exposed");
        if renderer.has_d3d_video_support() {
            assert!(
                renderer.mf_dxgi_device_manager().is_some(),
                "Video-capable D3D11 device must create IMFDXGIDeviceManager"
            );
            assert_ne!(renderer.mf_dxgi_reset_token(), 0);
        }
        let mut ui = AppUi::new(0);
        ui.attach_dxgi_device_manager(renderer.mf_dxgi_device_manager(), renderer.d3d_device());
        ui.attach_dxgi_device_manager(None, None);
        drop(ui);

        // Create a real single-slice GPU ID3D11Texture2D filled with cyan BGRA (240, 210, 20, 255)
        let mut cyan_init = vec![0u8; 256 * 256 * 4];
        for px in cyan_init.chunks_exact_mut(4) {
            px[0] = 240; // B
            px[1] = 210; // G
            px[2] = 20;  // R
            px[3] = 255;
        }
        let gpu_tex: ID3D11Texture2D = unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: 256,
                Height: 256,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let init_data = D3D11_SUBRESOURCE_DATA {
                pSysMem: cyan_init.as_ptr() as *const _,
                SysMemPitch: 256 * 4,
                SysMemSlicePitch: 0,
            };
            let mut out_opt: Option<ID3D11Texture2D> = None;
            d3d_dev
                .CreateTexture2D(&desc, Some(&init_data), Some(&mut out_opt))
                .expect("CreateTexture2D cyan");
            out_opt.expect("cyan texture")
        };

        renderer.reset_frame_stats();
        let dxva_snap = MotionFrameSnapshot {
            key: "u_r3_gpu_slot".to_string(),
            width: 256,
            height: 256,
            frame_seq: 1,
            bgra: Arc::new(Vec::new()), // intentionally empty for DXVA zero-copy!
            dxgi_texture: Some(gpu_tex.clone()),
            target_dim: 256,
        };
        let card_rect = RectF::new(100.0, 100.0, 220.0, 220.0);
        renderer.begin_draw(Color::rgb(16, 16, 19));
        assert!(
            renderer.draw_motion_artwork_alpha(&dxva_snap, card_rect, 12.0, 1.0),
            "DXVA zero-copy GPU texture draw must succeed even when bgra is empty"
        );
        renderer.end_window_draw_and_present(0.0, true, 0);

        let st_dxva = renderer.frame_stats();
        assert_eq!(st_dxva.motion_dxva_frames, 1);
        assert_eq!(
            st_dxva.motion_copy_from_memory_bytes, 0,
            "DXVA zero-copy path must incur 0 CopyFromMemory bytes"
        );
        assert_eq!(st_dxva.motion_cpu_fallback_frames, 0);

        let (rw, rh, px_dxva) = renderer.readback_backbuffer_bgra().expect("readback dxva");
        assert_eq!((rw, rh), (640, 640));
        let ci = ((210 * rw + 210) * 4) as usize;
        assert!(
            px_dxva[ci].abs_diff(240) <= 3
                && px_dxva[ci + 1].abs_diff(210) <= 3
                && px_dxva[ci + 2].abs_diff(20) <= 3,
            "DXVA surface must render cyan pixels accurately, got {:?}",
            &px_dxva[ci..ci + 4]
        );

        // Same key seamlessly transitions to downscaled 256x256 CPU fallback + opacity 0.5 crossfade!
        let mut blue_cpu = vec![0u8; 256 * 256 * 4];
        for px in blue_cpu.chunks_exact_mut(4) {
            px[0] = 240; // B
            px[1] = 20;  // G
            px[2] = 20;  // R
            px[3] = 255;
        }
        let cpu_snap = MotionFrameSnapshot {
            key: "u_r3_gpu_slot".to_string(),
            width: 256,
            height: 256,
            frame_seq: 2,
            bgra: Arc::new(blue_cpu),
            dxgi_texture: None,
            target_dim: 256,
        };
        renderer.begin_draw(Color::rgb(220, 20, 20)); // bright red static base
        assert!(renderer.draw_motion_artwork_alpha(&cpu_snap, card_rect, 12.0, 0.5));
        renderer.end_window_draw_and_present(0.0, true, 0);

        let st_cpu = renderer.frame_stats();
        assert_eq!(st_cpu.motion_cpu_fallback_frames, 1);
        assert_eq!(
            st_cpu.motion_copy_from_memory_bytes,
            (256 * 256 * 4) as u64,
            "Downscaled 256x256 CPU upload must be 262,144 bytes (3.6x less than 486x486)"
        );
        let (_, _, px_blend) = renderer.readback_backbuffer_bgra().expect("readback crossfade");
        // 50% red (B=20,G=20,R=220) + 50% blue (B=240,G=20,R=20) -> B~130, G~20, R~120
        assert!(
            px_blend[ci].abs_diff(130) <= 6
                && px_blend[ci + 1].abs_diff(20) <= 4
                && px_blend[ci + 2].abs_diff(120) <= 6,
            "50% motion crossfade must blend smoothly over static cover, got {:?}",
            &px_blend[ci..ci + 4]
        );

        // Verify 28-entry bounded LRU cap in D2DRenderer::motion_textures
        renderer.begin_draw(Color::rgb(16, 16, 19));
        for idx in 0..36 {
            let s = MotionFrameSnapshot {
                key: format!("u_lru_{idx}"),
                width: 256,
                height: 256,
                frame_seq: 1,
                bgra: Arc::new(Vec::new()),
                dxgi_texture: Some(gpu_tex.clone()),
                target_dim: 256,
            };
            assert!(renderer.draw_motion_artwork(&s, card_rect, 8.0, 1.0));
        }
        renderer.end_window_draw_and_present(0.0, true, 0);
        assert_eq!(
            renderer.motion_texture_cache_len(),
            MOTION_TEXTURE_LRU_CAP,
            "motion_textures LRU must stay bounded at 28 entries"
        );

        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn test_r4_cadence_alignment_dirty_rect_ring_and_r2_glass_synergy() {
        use am_light_lib::am_api::{EditorialItem, EditorialShelf, EditorialShelfKind};
        use am_light_lib::app_ui::{AppUi, PageRoute};
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::d2d_renderer::{
            compute_dirty_rects_union_area_px, dirty_area_exceeds_full_threshold,
            dirty_rects_bounding_box, merge_dirty_rect_ring_union, quantize_and_clamp_dirty_rects,
            D2DRenderer, DirtyFramePlan, DirtyRectPx, BACKDROP_DIRTY_TOP,
            DIRTY_RECT_FULL_FALLBACK_RATIO,
        };
        use am_light_lib::motion_art::cadence_ms_for_active_count;
        use am_light_lib::smtc::SmtcState;
        use am_light_lib::theme::{Color, RectF};
        use std::time::Duration;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        // 1. Pure R4 math: cadence thresholds, DPI quantization/clamping, 3-frame ring union, >55% threshold
        assert_eq!(cadence_ms_for_active_count(0), 33);
        assert_eq!(cadence_ms_for_active_count(4), 33);
        assert_eq!(cadence_ms_for_active_count(5), 42);
        assert_eq!(cadence_ms_for_active_count(20), 42);
        assert!((DIRTY_RECT_FULL_FALLBACK_RATIO - 0.55).abs() < 1e-6);

        let clamped = quantize_and_clamp_dirty_rects(
            &[
                RectF::new(-10.0, -5.0, 50.2, 40.3),
                RectF::new(980.0, 490.0, 100.0, 50.0),
                RectF::new(2000.0, 2000.0, 50.0, 50.0), // completely OOB
                RectF::new(10.0, 10.0, 0.0, 20.0),      // empty
            ],
            1.5,
            1000,
            500,
        );
        assert_eq!(clamped.len(), 1);
        assert_eq!(clamped[0], DirtyRectPx::new(0, 0, 61, 53));

        let d0 = vec![DirtyRectPx::new(10, 10, 110, 110)]; // 10,000 px
        let d1 = vec![DirtyRectPx::new(60, 10, 160, 110)]; // overlaps d0 by 50x100 -> union = 15,000 px
        let d2 = vec![DirtyRectPx::new(300, 200, 400, 300)]; // disjoint 10,000 px
        let ring_union = merge_dirty_rect_ring_union(&d0, &d1, &d2, 1000, 1000);
        assert_eq!(ring_union.len(), 2);
        assert_eq!(compute_dirty_rects_union_area_px(&ring_union), 25_000);
        assert_eq!(
            dirty_rects_bounding_box(&ring_union),
            Some(DirtyRectPx::new(10, 10, 400, 300))
        );
        assert!(
            !dirty_area_exceeds_full_threshold(&[DirtyRectPx::new(0, 0, 550, 1000)], 1000, 1000),
            "exact 55% must NOT trigger full fallback"
        );
        assert!(
            dirty_area_exceeds_full_threshold(&[DirtyRectPx::new(0, 0, 551, 1000)], 1000, 1000),
            ">55% must trigger full fallback"
        );

        // 2. Real D2DRenderer Present1 dirty-rect lifecycle, idle skip, >55% fallback, and R2 #58 cache invalidation
        unsafe extern "system" fn r4_wnd_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }
        let hwnd = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = w!("AMLightR4UnitHost");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(r4_wnd_proc),
                hInstance: hinst,
                lpszClassName: cls,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                cls,
                PCWSTR(w!("AM Light R4 Unit").as_ptr()),
                WS_POPUP,
                0,
                0,
                1400,
                880,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("CreateWindowExW R4")
        };

        let mut renderer = D2DRenderer::new(hwnd, 1400, 880, 1.0).expect("D2DRenderer::new R4");
        renderer.reset_frame_stats();
        assert_eq!(renderer.full_invalidation_frames_remaining(), 3);

        // First 3 frames initialize all 3 Flip-Model backbuffers via full Present
        let small_plan = DirtyFramePlan::with_rects(vec![RectF::new(100.0, 100.0, 200.0, 200.0)]);
        for _ in 0..3 {
            assert!(renderer.begin_draw_dirty(Color::rgb(16, 16, 19), &small_plan));
            renderer.fill_rect(RectF::new(100.0, 100.0, 200.0, 200.0), Color::rgb(40, 120, 220));
            renderer.end_window_draw_and_present(0.0, true, 0);
        }
        let st_init = renderer.frame_stats();
        assert_eq!(st_init.present_full_frames, 3);
        assert_eq!(st_init.present1_dirty_frames, 0);
        assert_eq!(renderer.full_invalidation_frames_remaining(), 0);

        // Frame 4: small dirty rect -> Present1 dirty presentation!
        assert!(renderer.begin_draw_dirty(Color::rgb(16, 16, 19), &small_plan));
        renderer.fill_rect(RectF::new(100.0, 100.0, 200.0, 200.0), Color::rgb(60, 160, 240));
        renderer.end_window_draw_and_present(0.0, true, 0);
        assert_eq!(renderer.frame_stats().present1_dirty_frames, 1);

        // Frame 5: empty idle plan -> begin_draw_dirty returns false & bumps skipped_idle_frames
        assert!(!renderer.begin_draw_dirty(Color::rgb(16, 16, 19), &DirtyFramePlan::idle()));
        assert_eq!(renderer.frame_stats().skipped_idle_frames, 1);
        assert_eq!(renderer.frame_stats().present1_dirty_frames, 1);

        // Frame 6: >55% dirty rect -> automatic full-frame fallback & resets invalidation ring
        let huge_plan = DirtyFramePlan::with_rects(vec![RectF::new(0.0, 0.0, 1100.0, 700.0)]);
        assert!(renderer.begin_draw_dirty(Color::rgb(16, 16, 19), &huge_plan));
        renderer.end_window_draw_and_present(0.0, true, 0);
        assert_eq!(renderer.frame_stats().present_full_frames, 4);
        assert_eq!(renderer.full_invalidation_frames_remaining(), 2);

        // Verify R2 #58 cache invalidation invariant: BACKDROP_DIRTY_TOP while top_bar_solid_bg=Some
        // must clear glass_cache_valid_top so subsequent top_bar_solid_bg=None + dirty_mask=0 re-blurs!
        renderer.begin_draw(Color::rgb(16, 16, 19));
        renderer.begin_glass_backdrop_snapshot_strips_ex(false, 54.0, 82.0, None, 0b11);
        renderer.end_glass_backdrop_snapshot();
        renderer.end_window_draw_and_present(0.0, true, 0);
        assert_eq!(renderer.frame_stats().glass_snapshot_blurs, 1);

        renderer.begin_draw(Color::rgb(16, 16, 19));
        renderer.begin_glass_backdrop_snapshot_strips_ex(
            false,
            54.0,
            82.0,
            Some(Color::rgb(16, 16, 19)),
            BACKDROP_DIRTY_TOP,
        );
        renderer.end_glass_backdrop_snapshot();
        renderer.end_window_draw_and_present(0.0, true, 0);

        renderer.begin_draw(Color::rgb(16, 16, 19));
        renderer.begin_glass_backdrop_snapshot_strips_ex(false, 54.0, 82.0, None, 0);
        renderer.end_glass_backdrop_snapshot();
        renderer.end_window_draw_and_present(0.0, true, 0);
        assert_eq!(
            renderer.frame_stats().glass_snapshot_blurs, 1,
            "Clearing glass_cache_valid_top on BACKDROP_DIRTY_TOP during solid fastpath must force re-blur when returning to live glass"
        );

        // 3. Settled Home CadenceStats, non-video redraw suppression, cadence lock, & dirty MiniPlayer present
        let mut ui = AppUi::new(0);
        ui.settings.playback_backend = PlaybackBackend::AppleMusicWeb;
        ui.settings.liquid_glass_enabled = true;
        ui.settings.reduce_transparency = false;
        ui.settings.reduce_motion = false;
        ui.settings.motion_artwork_enabled = true;
        ui.settings.live_eq_icon_spectrum = false;
        ui.settings.sidebar_collapsed = false;
        ui.is_window_focused = true;
        ui.route = PageRoute::Home;
        ui.show_lyrics_overlay = false;
        ui.route_trans = 1.0;
        ui.sidebar_open_spring.snap(1.0);
        ui.top_bar_guard_spring.snap(0.0);
        ui.mouse_pos = (-100.0, -100.0);
        let mk_smtc = |pos: f64| SmtcState {
            active: true,
            title: "Cadence Track".to_string(),
            artist: "AM Light".to_string(),
            status: "Playing".to_string(),
            position_sec: pos,
            duration_sec: 200.0,
            backend_mode: "web-am".to_string(),
            adam_id: Some("100001".to_string()),
            position_is_calibrated: true,
            ..Default::default()
        };
        ui.on_playback_state_changed(mk_smtc(30.0));
        ui.home_shelves = vec![
            EditorialShelf {
                title: "Top Picks".to_string(),
                subtitle: String::new(),
                kind: EditorialShelfKind::HomeTopPicks,
                items: (0..3)
                    .map(|i| EditorialItem {
                        id: format!("c_tp_{i}"),
                        kind: "album".to_string(),
                        play_kind: "album".to_string(),
                        title: format!("Pick {i}"),
                        subtitle: "Artist".to_string(),
                        motion_tall_video_url: Some(format!("https://example.invalid/cad_tp_{i}.m3u8")),
                        ..Default::default()
                    })
                    .collect(),
            },
            EditorialShelf {
                title: "Recently Played".to_string(),
                subtitle: String::new(),
                kind: EditorialShelfKind::SquareShelf,
                items: (0..4)
                    .map(|i| EditorialItem {
                        id: format!("c_sq_{i}"),
                        kind: "album".to_string(),
                        play_kind: "album".to_string(),
                        title: format!("Album {i}"),
                        subtitle: "Artist".to_string(),
                        motion_video_url: Some(format!("https://example.invalid/cad_sq_{i}.m3u8")),
                        ..Default::default()
                    })
                    .collect(),
            },
        ];

        // Prime 4 full frames so 3-buffer ring & settled Home baseline are initialized
        std::thread::sleep(Duration::from_millis(95));
        for _ in 0..4 {
            ui.step_and_render_ex(&mut renderer, 1.0 / 60.0, false);
            renderer.step_glass_animations(1.0);
        }
        // >=5 active Home motion slots -> effective_frame_pacing locks cleanly to 42ms / 24Hz!
        assert!(ui.active_motion_slot_urls().len() >= 5);
        assert_eq!(
            ui.effective_frame_pacing(),
            (0, 24, Some(42_000_000)),
            "Settled Home with >=5 active motion covers must lock pacing to 42ms/24Hz video cadence"
        );

        // Reset counters & step 10 settled playing frames with ZERO visual changes -> 100% idle-skipped!
        renderer.reset_frame_stats();
        ui.reset_cadence_stats();
        for _ in 0..10 {
            ui.step_and_render_ex(&mut renderer, 0.0, false);
        }
        let st_idle = renderer.frame_stats();
        let cad_idle = ui.cadence_stats();
        assert_eq!(
            st_idle.skipped_idle_frames, 10,
            "All 10 unchanged settled Home playing frames must idle-skip (0 redraw, 0 blur)"
        );
        assert_eq!(cad_idle.home_non_video_playing_redraws, 0);
        assert_eq!(cad_idle.home_full_redraw_count, 0);
        assert_eq!(st_idle.glass_snapshot_blurs, 0);

        // Advance playback position by 1.2s -> MiniPlayer progress pixel & second move -> surgical Present1 dirty frame!
        ui.on_playback_state_changed(mk_smtc(31.2));
        ui.step_and_render_ex(&mut renderer, 0.0, false);
        assert!(!ui.last_dirty_plan().force_full);
        assert!(
            !ui.last_dirty_plan().dirty_rects_dip.is_empty(),
            "MiniPlayer progress pixel change must produce targeted dirty rect"
        );
        assert!(
            renderer.frame_stats().present1_dirty_frames >= 1,
            "Settled Home progress tick must present via Present1 dirty rects"
        );
        assert!(
            ui.cadence_stats().home_dirty_rect_frames >= 1,
            "CadenceStats must record home_dirty_rect_frames"
        );
        assert_eq!(
            renderer.frame_stats().glass_snapshot_blurs, 0,
            "MiniPlayer progress tick must reuse cached backdrop with 0 glass blurs"
        );

        ui.motion_art_manager().set_global_enabled(false);
        drop(ui);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn test_bug1_apple_music_playlist_pagination_beyond_100_and_write_chunking() {
        use am_light_lib::am_api::{
            build_next_amp_path, build_offset_amp_path, build_offset_from_next_amp_path,
            chunk_library_playlist_song_ids, paginate_am_pages_with_fetcher, parse_amp_limit,
            parse_next_offset, DEFAULT_LIBRARY_WRITE_BATCH, DEFAULT_PLAYLIST_MAX_PAGES,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        assert_eq!(DEFAULT_PLAYLIST_MAX_PAGES, 100);
        assert_eq!(DEFAULT_LIBRARY_WRITE_BATCH, 50);

        let init_lib =
            "v1/me/library/playlists/p.99/tracks?include=catalog&extend=audioTraits&limit=100&l=zh-Hans-CN";
        assert_eq!(parse_amp_limit(init_lib), Some(100));
        assert_eq!(parse_next_offset("/v1/me/library/playlists/p.99/tracks?offset=200"), Some(200));

        // Guard against Apple's library quirk where `"next"` drops `/tracks` and query params
        let repaired_next = build_next_amp_path(init_lib, "/v1/me/library/playlists/p.99?offset=100");
        assert!(
            repaired_next.starts_with("v1/me/library/playlists/p.99/tracks?"),
            "repaired next must restore /tracks suffix: {repaired_next}"
        );
        assert!(repaired_next.contains("offset=100"));
        assert!(repaired_next.contains("include=catalog"));
        assert!(repaired_next.contains("extend=audioTraits"));
        assert!(repaired_next.contains("limit=100"));
        assert!(repaired_next.contains("l=zh-Hans-CN"));

        // Ensure `/tracks` without explicit limit still enforces `limit=100` on page 2+
        let enforce_limit = build_next_amp_path(
            "v1/catalog/cn/playlists/pl.abc/tracks?l=zh-Hans-CN",
            "/v1/catalog/cn/playlists/pl.abc/tracks?offset=100",
        );
        assert!(enforce_limit.contains("limit=100"));
        assert_eq!(parse_next_offset(&build_offset_amp_path(init_lib, 300)), Some(300));
        assert_eq!(
            parse_next_offset(&build_offset_from_next_amp_path(
                init_lib,
                Some("/v1/me/library/playlists/p.99?offset=100"),
                400,
            )),
            Some(400)
        );

        let make_items = |start: usize, count: usize| -> Vec<serde_json::Value> {
            (start..start + count)
                .map(|i| {
                    json!({
                        "id": format!("i.trk_{i}"),
                        "type": "library-songs",
                        "attributes": {
                            "name": format!("Track {i}"),
                            "artistName": format!("Artist {i}"),
                            "albumName": format!("Album {i}")
                        }
                    })
                })
                .collect()
        };

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio current_thread rt");

        rt.block_on(async {
            // 1. Library playlists > 100 tracks: 350 & 1,250 tracks via parallel offset fast-path
            for &total in &[350usize, 1250usize] {
                let first = make_items(0, 100);
                let out = paginate_am_pages_with_fetcher(
                    init_lib,
                    first,
                    Some("/v1/me/library/playlists/p.99/tracks?offset=100".to_string()),
                    Some(total),
                    DEFAULT_PLAYLIST_MAX_PAGES,
                    false,
                    move |path: String| async move {
                        let off = parse_next_offset(&path).unwrap_or(0);
                        let rem = total.saturating_sub(off).min(100);
                        let data = (off..off + rem)
                            .map(|i| {
                                json!({
                                    "id": format!("i.trk_{i}"),
                                    "type": "library-songs",
                                    "attributes": { "name": format!("Track {i}") }
                                })
                            })
                            .collect::<Vec<_>>();
                        let next = if off + rem < total {
                            Some(format!("/v1/me/library/playlists/p.99/tracks?offset={}", off + rem))
                        } else {
                            None
                        };
                        Ok(json!({
                            "data": data,
                            "next": next,
                            "meta": { "total": total }
                        }))
                    },
                )
                .await;
                assert_eq!(out.len(), total, "library playlist must paginate all {total} tracks");
                for (idx, item) in out.iter().enumerate() {
                    assert_eq!(item.get("id").and_then(|v| v.as_str()), Some(format!("i.trk_{idx}").as_str()));
                }
            }

            // 2. Catalog playlist inline 100 + `/tracks?offset=100` sequential cursor (no meta.total, 325 tracks)
            let cat_init =
                "v1/catalog/cn/playlists/pl.cat1/tracks?extend=audioTraits&limit=100&l=zh-Hans-CN";
            let cat_out = paginate_am_pages_with_fetcher(
                cat_init,
                make_items(0, 100),
                Some("/v1/catalog/cn/playlists/pl.cat1/tracks?offset=100".to_string()),
                None,
                DEFAULT_PLAYLIST_MAX_PAGES,
                false,
                move |path: String| async move {
                    assert!(path.contains("extend=audioTraits"));
                    assert!(path.contains("limit=100"));
                    let off = parse_next_offset(&path).unwrap_or(0);
                    let total = 325usize;
                    let rem = total.saturating_sub(off).min(100);
                    let data = (off..off + rem)
                        .map(|i| json!({ "id": format!("i.trk_{i}"), "type": "songs" }))
                        .collect::<Vec<_>>();
                    let next = if off + rem < total {
                        Some(format!("/v1/catalog/cn/playlists/pl.cat1/tracks?offset={}", off + rem))
                    } else {
                        None
                    };
                    Ok(json!({ "data": data, "next": next }))
                },
            )
            .await;
            assert_eq!(cat_out.len(), 325);
            assert_eq!(cat_out[324].get("id").and_then(|v| v.as_str()), Some("i.trk_324"));

            // 3. Partial failure in 4-way parallel batch (`offset=200` fails once with 429 while 100 & 300 succeed):
            // must recover all 350 tracks and reuse `batch_cache` for offset=300 without re-fetching it!
            let off200_calls = Arc::new(AtomicUsize::new(0));
            let off300_calls = Arc::new(AtomicUsize::new(0));
            let c200 = Arc::clone(&off200_calls);
            let c300 = Arc::clone(&off300_calls);
            let recovered = paginate_am_pages_with_fetcher(
                init_lib,
                make_items(0, 100),
                Some("/v1/me/library/playlists/p.99/tracks?offset=100".to_string()),
                Some(350),
                DEFAULT_PLAYLIST_MAX_PAGES,
                false,
                move |path: String| {
                    let c200 = Arc::clone(&c200);
                    let c300 = Arc::clone(&c300);
                    async move {
                        let off = parse_next_offset(&path).unwrap_or(0);
                        if off == 200 {
                            if c200.fetch_add(1, Ordering::SeqCst) == 0 {
                                return Err("HTTP 429 Too Many Requests".to_string());
                            }
                        } else if off == 300 {
                            c300.fetch_add(1, Ordering::SeqCst);
                        }
                        let rem = 350usize.saturating_sub(off).min(100);
                        let data = (off..off + rem)
                            .map(|i| json!({ "id": format!("i.trk_{i}") }))
                            .collect::<Vec<_>>();
                        let next = if off + rem < 350 {
                            Some(format!("/v1/me/library/playlists/p.99/tracks?offset={}", off + rem))
                        } else {
                            None
                        };
                        Ok(json!({ "data": data, "next": next, "meta": { "total": 350 } }))
                    }
                },
            )
            .await;
            assert_eq!(recovered.len(), 350, "partial parallel batch failure must not truncate at 100/200");
            assert_eq!(off200_calls.load(Ordering::SeqCst), 2, "failed offset=200 must be retried sequentially");
            assert_eq!(off300_calls.load(Ordering::SeqCst), 1, "offset=300 must be served from batch_cache");
            for (idx, item) in recovered.iter().enumerate() {
                assert_eq!(item.get("id").and_then(|v| v.as_str()), Some(format!("i.trk_{idx}").as_str()));
            }

            // 4a. Omitted `"next"` and `meta.total` at 100 tracks on a 250-track playlist -> implicit offset probe
            let probed_250 = paginate_am_pages_with_fetcher(
                cat_init,
                make_items(0, 100),
                None,
                None,
                DEFAULT_PLAYLIST_MAX_PAGES,
                false,
                move |path: String| async move {
                    let off = parse_next_offset(&path).unwrap_or(0);
                    let rem = 250usize.saturating_sub(off).min(100);
                    let data = (off..off + rem)
                        .map(|i| json!({ "id": format!("i.trk_{i}") }))
                        .collect::<Vec<_>>();
                    Ok(json!({ "data": data }))
                },
            )
            .await;
            assert_eq!(probed_250.len(), 250, "implicit offset probe must fetch all 250 tracks when next is omitted");

            // 4b. Playlist truly has 100 tracks -> probe at offset=100 gets HTTP 404 and stops cleanly at 100
            let exact_100 = paginate_am_pages_with_fetcher(
                cat_init,
                make_items(0, 100),
                None,
                None,
                DEFAULT_PLAYLIST_MAX_PAGES,
                false,
                move |_path: String| async move { Err("HTTP 404 Not Found".to_string()) },
            )
            .await;
            assert_eq!(exact_100.len(), 100);

            // 4c. Server ignores offset and repeats page 1 -> `page_fingerprint` stops cleanly without duplicating
            let dup_calls = Arc::new(AtomicUsize::new(0));
            let dup_c = Arc::clone(&dup_calls);
            let repeat_guard = paginate_am_pages_with_fetcher(
                cat_init,
                make_items(0, 100),
                None,
                None,
                DEFAULT_PLAYLIST_MAX_PAGES,
                false,
                move |_path: String| {
                    let dup_c = Arc::clone(&dup_c);
                    async move {
                        dup_c.fetch_add(1, Ordering::SeqCst);
                        let data = (0..100)
                            .map(|i| json!({ "id": format!("i.trk_{i}") }))
                            .collect::<Vec<_>>();
                        Ok(json!({ "data": data }))
                    }
                },
            )
            .await;
            assert_eq!(repeat_guard.len(), 100);
            assert_eq!(dup_calls.load(Ordering::SeqCst), 1, "page_fingerprint must halt on first repeated page");

            // 5. HTTP 400 fallback when page 2+ rejects `include=catalog`/`extend=audioTraits`
            let fallback_400 = paginate_am_pages_with_fetcher(
                init_lib,
                make_items(0, 100),
                Some("/v1/me/library/playlists/p.99?offset=100".to_string()),
                None,
                DEFAULT_PLAYLIST_MAX_PAGES,
                false,
                move |path: String| async move {
                    if path.contains("include=catalog") || path.contains("extend=audioTraits") {
                        return Err("HTTP 400 Bad Request".to_string());
                    }
                    assert!(path.starts_with("v1/me/library/playlists/p.99/tracks?"));
                    let off = parse_next_offset(&path).unwrap_or(0);
                    let rem = 220usize.saturating_sub(off).min(100);
                    let data = (off..off + rem)
                        .map(|i| json!({ "id": format!("i.trk_{i}") }))
                        .collect::<Vec<_>>();
                    let next = if off + rem < 220 {
                        Some(format!("/v1/me/library/playlists/p.99?offset={}", off + rem))
                    } else {
                        None
                    };
                    Ok(json!({ "data": data, "next": next }))
                },
            )
            .await;
            assert_eq!(fallback_400.len(), 220, "HTTP 400 stripped-param fallback must paginate past 100");
        });

        // 6. Playlist transfer/write chunking: 275 valid IDs + invalid noise -> batches of 50 without 100-cap
        let mut raw_ids: Vec<String> = (0..275)
            .map(|i| {
                if i % 5 == 0 {
                    format!("  i.lib_{i}  ")
                } else {
                    format!("14408{:05}", i)
                }
            })
            .collect();
        raw_ids.insert(10, "   ".to_string());
        raw_ids.insert(77, "bad id/with spaces".to_string());
        let batches = chunk_library_playlist_song_ids(&raw_ids, DEFAULT_LIBRARY_WRITE_BATCH);
        let lens: Vec<usize> = batches.iter().map(Vec::len).collect();
        assert_eq!(lens, vec![50, 50, 50, 50, 50, 25]);
        let flat: Vec<&str> = batches.iter().flatten().map(String::as_str).collect();
        assert_eq!(flat.len(), 275);
        assert_eq!(flat[0], "i.lib_0");
        assert_eq!(flat[274], "1440800274");
    }

    #[test]
    fn test_bug2_netease_playlist_sync_beyond_200_and_cover_accuracy() {
        use am_light_lib::netease_api::{
            build_song_detail_c_param, compute_missing_playlist_track_ids, cover_url_from_pic_id,
            extract_playlist_track_ids, is_netease_placeholder_cover,
            merge_playlist_tracks_in_order, order_tracks_by_ids, parse_ne_song,
            parse_song_detail_response, plan_song_detail_chunks, NE_PLAYLIST_MAX_TRACKS,
            NE_SONG_DETAIL_CHUNK_SIZE,
        };

        assert_eq!(NE_SONG_DETAIL_CHUNK_SIZE, 300);
        assert_eq!(NE_PLAYLIST_MAX_TRACKS, 5000);

        // 1. trackIds extraction, deduplication, and 5000 safe cap across 550, 1250, and 6000 IDs
        let mut raw_tids_550: Vec<serde_json::Value> =
            (1..=550i64).map(|id| json!({ "id": id })).collect();
        raw_tids_550.push(json!({ "id": 42 })); // duplicate
        raw_tids_550.push(json!({ "id": 0 })); // invalid
        raw_tids_550.push(json!({ "id": -7 })); // invalid
        let ids_550 =
            extract_playlist_track_ids(&json!({ "trackIds": raw_tids_550 }), NE_PLAYLIST_MAX_TRACKS);
        assert_eq!(ids_550.len(), 550);
        assert_eq!(
            plan_song_detail_chunks(&ids_550, NE_SONG_DETAIL_CHUNK_SIZE, NE_PLAYLIST_MAX_TRACKS)
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![300, 250]
        );

        let ids_1250 = extract_playlist_track_ids(
            &json!({ "trackIds": (1..=1250i64).map(|id| json!({ "id": id })).collect::<Vec<_>>() }),
            NE_PLAYLIST_MAX_TRACKS,
        );
        assert_eq!(ids_1250.len(), 1250);
        assert_eq!(
            plan_song_detail_chunks(&ids_1250, NE_SONG_DETAIL_CHUNK_SIZE, NE_PLAYLIST_MAX_TRACKS)
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            vec![300, 300, 300, 300, 50]
        );

        let ids_6000 = extract_playlist_track_ids(
            &json!({ "trackIds": (1..=6000i64).map(|id| json!({ "id": id })).collect::<Vec<_>>() }),
            NE_PLAYLIST_MAX_TRACKS,
        );
        assert_eq!(ids_6000.len(), 5000, "playlist extraction must cap safely at NE_PLAYLIST_MAX_TRACKS");
        assert_eq!(build_song_detail_c_param(&[101, 202, 303]), r#"[{"id":101},{"id":202},{"id":303}]"#);

        // 2. 200 inline tracks + 650 non-monotonic `trackIds` -> hydrate remaining 450 + 3 artwork-less inline
        // tracks across out-of-order chunks, preserving exact `trackIds` order & unique covers!
        let ordered_650: Vec<i64> = (0..650i64)
            .map(|i| if i % 2 == 0 { 500_000 - i } else { 100_000 + i })
            .collect();
        let make_ne_song_json = |id: i64, pic_url: &str| -> serde_json::Value {
            json!({
                "id": id,
                "name": format!("NE Song {id}"),
                "ar": [{ "name": format!("NE Artist {id}") }],
                "al": { "name": format!("NE Album {id}"), "picUrl": pic_url },
                "dt": 215000
            })
        };

        let inline_200: Vec<_> = ordered_650[..200]
            .iter()
            .enumerate()
            .filter_map(|(idx, &id)| {
                let pic = if matches!(idx, 5 | 55 | 155) {
                    String::new() // missing cover inline -> must be queued for hydration!
                } else {
                    format!("http://p1.music.126.net/inline_{id}/{id}.jpg")
                };
                parse_ne_song(&make_ne_song_json(id, &pic), None)
            })
            .collect();
        assert_eq!(inline_200.len(), 200);
        assert!(inline_200[5].artwork_url.is_none());

        let missing = compute_missing_playlist_track_ids(&ordered_650, &inline_200);
        assert_eq!(
            missing.len(),
            453,
            "must request all 450 unhydrated tracks + 3 inline tracks missing artwork (zero 200-cap!)"
        );
        assert!(missing.contains(&ordered_650[5]));
        assert!(missing.contains(&ordered_650[55]));
        assert!(missing.contains(&ordered_650[155]));
        assert!(missing.contains(&ordered_650[649]));

        // Simulate out-of-order `/api/v3/song/detail` batch responses (reversed chunks)
        let mut fetched_all = Vec::new();
        for chunk in plan_song_detail_chunks(&missing, NE_SONG_DETAIL_CHUNK_SIZE, NE_PLAYLIST_MAX_TRACKS)
            .into_iter()
            .rev()
        {
            let songs_json: Vec<serde_json::Value> = chunk
                .iter()
                .rev()
                .map(|&id| make_ne_song_json(id, &format!("http://p2.music.126.net/detail_{id}/{id}.jpg")))
                .collect();
            fetched_all.extend(parse_song_detail_response(&json!({ "code": 200, "songs": songs_json })));
        }
        assert_eq!(fetched_all.len(), 453);

        let merged = merge_playlist_tracks_in_order(&ordered_650, inline_200.clone(), fetched_all.clone());
        assert_eq!(merged.len(), 650, "merged NetEase playlist must retain all 650 tracks");
        for (idx, (tr, &want_id)) in merged.iter().zip(ordered_650.iter()).enumerate() {
            assert_eq!(tr.id, format!("ne:{want_id}"), "exact trackIds order broken at idx {idx}");
            let art = tr
                .artwork_url
                .as_deref()
                .expect("every merged track must carry its own album cover");
            assert!(
                art.starts_with("https://") && art.contains(&format!("/{want_id}.jpg?param=480y480")),
                "track {want_id} cover bleed or missing https upgrade: {art}"
            );
        }
        // Inline tracks 5, 55, 155 must be upgraded from `song/detail` without altering other tracks
        assert!(merged[5].artwork_url.as_deref().unwrap().contains("detail_"));
        assert!(merged[0].artwork_url.as_deref().unwrap().contains("inline_"));

        // 3. Partial chunk failure resilience: dropping 100 tracks from `fetched_all` still preserves 550 in exact order
        let partial_fetched = fetched_all[100..].to_vec();
        let partial_merged = merge_playlist_tracks_in_order(&ordered_650, inline_200, partial_fetched);
        assert_eq!(partial_merged.len(), 550);
        let mut last_pos = None;
        for tr in &partial_merged {
            let id_num: i64 = tr.id.strip_prefix("ne:").unwrap().parse().unwrap();
            let pos = ordered_650.iter().position(|&x| x == id_num).unwrap();
            if let Some(lp) = last_pos {
                assert!(pos > lp, "partial failure merge must keep strictly increasing trackIds order");
            }
            last_pos = Some(pos);
        }

        // `order_tracks_by_ids` deduplicates and prefers entry with artwork
        let no_art = parse_ne_song(&make_ne_song_json(777, ""), None).unwrap();
        let with_art = parse_ne_song(&make_ne_song_json(777, "https://p1.music.126.net/ok/777.jpg"), None).unwrap();
        let ordered_dedup = order_tracks_by_ids(&[777], vec![no_art, with_art]);
        assert_eq!(ordered_dedup.len(), 1);
        assert!(ordered_dedup[0].artwork_url.as_deref().is_some_and(|u| u.contains("/ok/777.jpg")));

        // 4. NetEase cover URL accuracy, placeholder rejection, protocol upgrade, and pic_str derivation
        assert!(is_netease_placeholder_cover(""));
        assert!(is_netease_placeholder_cover("https://p1.music.126.net/abc/5639395138885805.jpg"));
        assert!(!is_netease_placeholder_cover("https://p1.music.126.net/abc/109951166952713766.jpg"));

        assert_eq!(
            cover_url_from_pic_id("109951166952713766", 480).as_deref(),
            Some("https://p1.music.126.net/pcYHpMkdC69VVvWiynNklA==/109951166952713766.jpg?param=480y480")
        );
        assert_eq!(cover_url_from_pic_id("0", 480), None);
        assert_eq!(cover_url_from_pic_id("5639395138885805", 480), None);
        assert_eq!(cover_url_from_pic_id("not-numeric", 480), None);

        // `parse_ne_song` with `None` fallback never inherits playlist/artist cover & never returns `Some("")`
        let bare = parse_ne_song(
            &json!({ "id": 1001, "name": "Bare", "ar": [{"name": "A"}], "al": { "name": "Al", "picUrl": "   " } }),
            None,
        )
        .unwrap();
        assert_eq!(bare.artwork_url, None);

        // Rejects placeholder `picUrl` and falls through to `blurPicUrl`, upgrading `//` -> `https://`
        let placeholder_fallback = parse_ne_song(
            &json!({
                "id": 1002,
                "name": "PlaceholderFallthrough",
                "ar": [{"name": "B"}],
                "al": {
                    "name": "Alb",
                    "picUrl": "https://p1.music.126.net/x/5639395138885805.jpg",
                    "blurPicUrl": "//p2.music.126.net/real/999888.jpg"
                }
            }),
            None,
        )
        .unwrap();
        assert_eq!(
            placeholder_fallback.artwork_url.as_deref(),
            Some("https://p2.music.126.net/real/999888.jpg?param=480y480")
        );

        // Derives exact CDN URL from `al.pic_str` when `picUrl` & `blurPicUrl` are omitted
        let from_pic_str = parse_ne_song(
            &json!({
                "id": 1003,
                "name": "PicStrSong",
                "ar": [{"name": "C"}],
                "al": { "name": "AlbC", "pic_str": "109951166952713766" }
            }),
            None,
        )
        .unwrap();
        assert_eq!(
            from_pic_str.artwork_url.as_deref(),
            Some("https://p1.music.126.net/pcYHpMkdC69VVvWiynNklA==/109951166952713766.jpg?param=480y480")
        );
    }

    #[test]
    fn test_bug3_cover_artwork_accuracy_smtc_renderer_and_app_ui() {
        use am_light_lib::am_agent::{
            should_alias_cached_track_meta, AgentTrackInput, CachedTrackMeta,
        };
        use am_light_lib::am_api::{
            artwork_canonical_stem, is_apple_catalog_numeric_id, motion_album_candidate_matches,
        };
        use am_light_lib::app_ui::{
            make_smtc_artwork_key, meta_motion_cache_key, smtc_catalog_cache_key,
            upgrade_artwork_request_url, AppUi, PLAYLIST_CACHE_SCHEMA_VERSION,
        };
        use am_light_lib::d2d_renderer::D2DRenderer;
        use am_light_lib::motion_art::MotionFrameSnapshot;
        use am_light_lib::smtc::{smtc_album_identity_key, SmtcThumbCacheState};
        use am_light_lib::theme::{Color, RectF};
        use std::sync::Arc;
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        // --- A. Apple Music stem canonicalization, numeric ID guard, motion validation & cache aliasing ---
        assert_eq!(
            artwork_canonical_stem("https://is1-ssl.mzstatic.com/image/thumb/Music126/v4/aa/bb/cc/240x240bb.jpg"),
            artwork_canonical_stem("https://is1-ssl.mzstatic.com/image/thumb/Music126/v4/aa/bb/cc/600x600bb.jpg"),
            "same mzstatic cover at different sizes must share canonical stem"
        );
        // Generated Apple playlist URLs with query params MUST stay distinct (never collapse to `.../gen`)
        let gen_a = "https://is1-ssl.mzstatic.com/image/thumb/gen/1200x1200AM.PDCXS19.jpg?c1=aaa&t=1";
        let gen_b = "https://is1-ssl.mzstatic.com/image/thumb/gen/1200x1200AM.PDCXS11.jpg?c1=bbb&t=2";
        assert_ne!(artwork_canonical_stem(gen_a), artwork_canonical_stem(gen_b));
        assert_eq!(artwork_canonical_stem(gen_a), gen_a);
        // NetEase query URLs, non-dimension numeric filenames, and smtc:// keys MUST NEVER collide
        let ne_a = "https://p1.music.126.net/hashA/109951168912345678.jpg?param=300x300";
        let ne_b = "https://p1.music.126.net/hashB/109951168999999999.jpg?param=300x300";
        assert_ne!(artwork_canonical_stem(ne_a), artwork_canonical_stem(ne_b));
        assert_ne!(
            artwork_canonical_stem("https://cdn.example.com/123456_extra.jpg"),
            artwork_canonical_stem("https://cdn.example.com/123456_other.jpg")
        );
        assert_eq!(
            artwork_canonical_stem("smtc://12345x678/song"),
            "smtc://12345x678/song"
        );

        assert!(is_apple_catalog_numeric_id("1440818842"));
        assert!(!is_apple_catalog_numeric_id("ne:12345"));
        assert!(!is_apple_catalog_numeric_id("qq:001abc"));
        assert!(!is_apple_catalog_numeric_id("i.lib123"));
        assert!(!is_apple_catalog_numeric_id(""));

        assert!(motion_album_candidate_matches(
            "晴天",
            "周杰伦",
            "叶惠美",
            "叶惠美",
            "周杰伦"
        ));
        assert!(motion_album_candidate_matches(
            "Anti-Hero",
            "Taylor Swift",
            "Midnights",
            "Midnights (3am Edition)",
            "Taylor Swift"
        ));
        assert!(!motion_album_candidate_matches(
            "晴天",
            "周杰伦",
            "叶惠美",
            "叶惠美",
            "Wrong Artist"
        ));
        assert!(!motion_album_candidate_matches(
            "晴天",
            "周杰伦",
            "叶惠美",
            "范特西",
            "周杰伦"
        ));
        assert!(!motion_album_candidate_matches(
            "Hello",
            "Adele",
            "",
            "Unrelated Greatest Hits",
            "Adele"
        ));
        assert!(motion_album_candidate_matches(
            "Hello",
            "Adele",
            "",
            "Hello - Single",
            "Adele"
        ));

        let meta_a = CachedTrackMeta {
            title: "Song A".to_string(),
            artist: "Artist A".to_string(),
            album: "Album A".to_string(),
            duration_sec: 200.0,
            artwork_url: Some("https://is1-ssl.mzstatic.com/a/600x600bb.jpg".to_string()),
            audio_quality: None,
        };
        assert!(
            should_alias_cached_track_meta("i.lib123", "1440818842", &meta_a, Some("Song A"), Some("Artist A")),
            "library ID -> catalog ID for same song must be allowed"
        );
        assert!(
            !should_alias_cached_track_meta("111", "222", &meta_a, None, None),
            "distinct numeric catalog IDs without title proof must NOT alias (prevents cover poisoning)"
        );
        assert!(
            !should_alias_cached_track_meta("111", "222", &meta_a, Some("Song B"), Some("Artist B")),
            "different incoming track must NEVER inherit previous track's cached cover"
        );
        assert!(
            !should_alias_cached_track_meta("i.lib123", "222", &meta_a, Some("Song B"), Some("Artist B")),
            "even library ID must NOT alias if incoming title explicitly differs"
        );

        // `parse_track_from_am_json` ignores empty `""` artwork/album and respects `None` playlist fallback
        let am_empty_art = json!({
            "id": "i.noart",
            "attributes": {
                "name": "NoArtTrack",
                "artistName": "SomeArtist",
                "albumName": "   ",
                "artwork": { "url": "   " }
            }
        });
        let parsed_pl_tr = parse_track_from_am_json(&am_empty_art, None, None).unwrap();
        assert_eq!(parsed_pl_tr.artwork_url, None);
        assert!(parsed_pl_tr.album_name.is_empty());

        // --- B. SMTC cross-track stale thumbnail guard & same-album zero-flicker acceptance ---
        assert_eq!(
            smtc_album_identity_key("Track 1", "Artist X", "Album Z"),
            smtc_album_identity_key("Track 2", "Artist X", "Album Z")
        );
        assert_ne!(
            smtc_album_identity_key("Track 1", "Artist X", ""),
            smtc_album_identity_key("Track 2", "Artist X", ""),
            "empty album must fall back to track identity"
        );

        let mut tc = SmtcThumbCacheState::default();
        let key_a = "Song A\x1fArtist A\x1fAlbA";
        let key_b = "Song B\x1fArtist B\x1fAlbB";
        let key_c = "Song C\x1fArtist B\x1fAlbB"; // same album as Song B
        let key_d = "Song D\x1fArtist D\x1fAlbD"; // different album with genuinely identical bytes

        assert!(tc.should_read_thumbnail(key_a, false));
        let got_a = tc.commit_read_thumbnail(key_a, "Song A", "Artist A", "AlbA", Some("B64_A".to_string()), 1_000);
        assert_eq!(got_a.as_deref(), Some("B64_A"));
        assert!(!tc.is_awaiting_fresh_thumb());
        assert!(!tc.should_read_thumbnail(key_a, false));

        // Song A -> Song B (different album) returning stale `B64_A` at t + 100ms -> REJECTED!
        let stale_b = tc.commit_read_thumbnail(key_b, "Song B", "Artist B", "AlbB", Some("B64_A".to_string()), 1_100);
        assert_eq!(stale_b, None, "stale previous-track SMTC bytes must be rejected across album switch");
        assert!(tc.is_awaiting_fresh_thumb());
        assert!(tc.should_read_thumbnail(key_b, false));

        // Song B receives real `B64_B` at t + 350ms -> ACCEPTED!
        let fresh_b = tc.commit_read_thumbnail(key_b, "Song B", "Artist B", "AlbB", Some("B64_B".to_string()), 1_350);
        assert_eq!(fresh_b.as_deref(), Some("B64_B"));
        assert!(!tc.is_awaiting_fresh_thumb());
        assert!(!tc.should_read_thumbnail(key_b, false));

        // `MediaPropertiesChanged` (`media_dirty = true`) forces re-read and accepts upgraded `B64_B_v2`
        assert!(tc.should_read_thumbnail(key_b, true));
        let upgraded_b = tc.commit_read_thumbnail(key_b, "Song B", "Artist B", "AlbB", Some("B64_B_v2".to_string()), 1_600);
        assert_eq!(upgraded_b.as_deref(), Some("B64_B_v2"));

        // Song B -> Song C on SAME non-empty album (`AlbB`): identical bytes accepted IMMEDIATELY (0ms flicker)!
        let same_alb_c = tc.commit_read_thumbnail(key_c, "Song C", "Artist B", "AlbB", Some("B64_B_v2".to_string()), 1_650);
        assert_eq!(same_alb_c.as_deref(), Some("B64_B_v2"));
        assert!(!tc.is_awaiting_fresh_thumb());

        // Song C -> Song D (different album) with genuinely identical bytes: rejected < 2500ms, accepted >= 2500ms
        let rej_d = tc.commit_read_thumbnail(key_d, "Song D", "Artist D", "AlbD", Some("B64_B_v2".to_string()), 2_000);
        assert_eq!(rej_d, None);
        assert!(tc.is_awaiting_fresh_thumb());
        let acc_d = tc.commit_read_thumbnail(key_d, "Song D", "Artist D", "AlbD", Some("B64_B_v2".to_string()), 4_600);
        assert_eq!(acc_d.as_deref(), Some("B64_B_v2"));
        assert!(!tc.is_awaiting_fresh_thumb());

        // --- C. app_ui schema version, cache keys, URL upgrade safety & optimistic track switch ---
        assert_eq!(PLAYLIST_CACHE_SCHEMA_VERSION, 3, "schema v3 forces legacy truncated v2 caches to revalidate");
        assert_ne!(
            make_smtc_artwork_key("Song", "Artist", "Album", "B64_V1"),
            make_smtc_artwork_key("Song", "Artist", "Album", "B64_V2")
        );
        assert_ne!(
            make_smtc_artwork_key("Song", "Artist", "Album 1", "SAME"),
            make_smtc_artwork_key("Song", "Artist", "Album 2", "SAME")
        );
        assert_ne!(
            smtc_catalog_cache_key("Title", "Artist", "Single"),
            smtc_catalog_cache_key("Title", "Artist", "Album")
        );
        assert_ne!(
            meta_motion_cache_key("Title", "Artist", "Single"),
            meta_motion_cache_key("Title", "Artist", "Album")
        );

        assert_eq!(
            upgrade_artwork_request_url("https://is1-ssl.mzstatic.com/image/thumb/a/360x360bb.jpg", true),
            "https://is1-ssl.mzstatic.com/image/thumb/a/720x720bb.jpg"
        );
        assert_eq!(
            upgrade_artwork_request_url("https://is1-ssl.mzstatic.com/image/thumb/a/360x360bb.jpg", false),
            "https://is1-ssl.mzstatic.com/image/thumb/a/480x480bb.jpg"
        );
        let gen_q = "https://is1-ssl.mzstatic.com/image/thumb/gen/360x360AM.PDCXS.jpg?c1=1";
        let ne_u = "https://p1.music.126.net/hash/360x360_real.jpg?param=480y480";
        let qq_u = "https://y.gtimg.cn/music/photo_new/T002R500x500M000abc.jpg";
        assert_eq!(upgrade_artwork_request_url(gen_q, true), gen_q);
        assert_eq!(upgrade_artwork_request_url(ne_u, true), ne_u);
        assert_eq!(upgrade_artwork_request_url(qq_u, true), qq_u);

        // `begin_optimistic_playback_loading` must clear prior track's cover & SMTC bytes even when new track has `artwork_url: None`
        let mut ui = AppUi::new(0);
        ui.smtc.artwork_url = Some("https://is1-ssl.mzstatic.com/old/600x600bb.jpg".to_string());
        ui.smtc.thumbnail_base64 = Some("STALE_OLD_THUMB".to_string());
        let tr_no_art = AgentTrackInput {
            id: "ne:9001".to_string(),
            catalog_id: Some("9001".to_string()),
            kind: Some("ne-song".to_string()),
            name: "No Cover Track".to_string(),
            artist_name: "Artist".to_string(),
            album_name: "Album".to_string(),
            duration_sec: 180.0,
            artwork_url: None,
            audio_traits: Vec::new(),
        };
        ui.begin_optimistic_playback_loading(Some(&tr_no_art), None);
        assert_eq!(ui.smtc.artwork_url, None, "switching to artwork_url=None must clear previous cover");
        assert_eq!(ui.smtc.thumbnail_base64, None, "optimistic switch must clear stale SMTC thumbnail bytes");

        let tr_with_art = AgentTrackInput {
            artwork_url: Some("https://p1.music.126.net/new/9002.jpg?param=480y480".to_string()),
            ..tr_no_art
        };
        ui.begin_optimistic_playback_loading(Some(&tr_with_art), None);
        assert_eq!(
            ui.smtc.artwork_url.as_deref(),
            Some("https://p1.music.126.net/new/9002.jpg?param=480y480")
        );
        drop(ui);

        // --- D. D2DRenderer CPU-fallback motion texture Arc::ptr_eq & invalidate_motion_texture ---
        unsafe extern "system" fn bug3_wnd_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }
        let hwnd = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = w!("AMLightBug3UnitHost");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(bug3_wnd_proc),
                hInstance: hinst,
                lpszClassName: cls,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                cls,
                PCWSTR(w!("AM Light Bug3 Unit").as_ptr()),
                WS_POPUP,
                0,
                0,
                320,
                320,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("CreateWindowExW Bug3")
        };

        let mut renderer = D2DRenderer::new(hwnd, 320, 320, 1.0).expect("D2DRenderer::new Bug3");
        renderer.reset_frame_stats();
        let rect = RectF::new(20.0, 20.0, 160.0, 160.0);
        let frame_a = MotionFrameSnapshot {
            key: "lyrics_cover".to_string(),
            width: 64,
            height: 64,
            frame_seq: 1,
            bgra: Arc::new(vec![0xAA; 64 * 64 * 4]),
            dxgi_texture: None,
            target_dim: 64,
        };
        let frame_b = MotionFrameSnapshot {
            key: "lyrics_cover".to_string(),
            width: 64,
            height: 64,
            frame_seq: 1, // identical frame_seq & dims, distinct Arc buffer (e.g. new song!)
            bgra: Arc::new(vec![0x55; 64 * 64 * 4]),
            dxgi_texture: None,
            target_dim: 64,
        };

        renderer.begin_draw(Color::rgb(12, 12, 16));
        assert!(renderer.draw_motion_artwork(&frame_a, rect, 12.0, 1.0));
        renderer.end_window_draw_and_present(0.0, true, 0);
        assert_eq!(renderer.frame_stats().motion_cpu_fallback_frames, 1);

        // Same Arc + same frame_seq -> no redundant CPU re-upload
        renderer.begin_draw(Color::rgb(12, 12, 16));
        assert!(renderer.draw_motion_artwork(&frame_a, rect, 12.0, 1.0));
        renderer.end_window_draw_and_present(0.0, true, 0);
        assert_eq!(renderer.frame_stats().motion_cpu_fallback_frames, 1);

        // Distinct Arc buffer with SAME frame_seq=1 -> MUST upload fresh pixels (no stale cover bleed!)
        renderer.begin_draw(Color::rgb(12, 12, 16));
        assert!(renderer.draw_motion_artwork(&frame_b, rect, 12.0, 1.0));
        renderer.end_window_draw_and_present(0.0, true, 0);
        assert_eq!(
            renderer.frame_stats().motion_cpu_fallback_frames,
            2,
            "distinct Arc<Vec<u8>> with same frame_seq must re-upload instead of showing stale song motion cover"
        );
        assert_eq!(renderer.motion_texture_cache_len(), 1);

        renderer.invalidate_motion_texture("lyrics_cover");
        assert_eq!(
            renderer.motion_texture_cache_len(),
            0,
            "invalidate_motion_texture must drop cached GPU texture entry"
        );

        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn test_phase2_perf_ui_revisions_zero_clone_idle_cadence_and_search() {
        use am_light_lib::am_api::{EditorialItem, EditorialShelf, EditorialShelfKind};
        use am_light_lib::app_ui::{
            contains_ascii_icase, upgrade_artwork_request_url_dim, AppUi, CachedPlaylistDetail,
            PageRoute, PlaylistDiskCache, SharedPlaylistDetail,
        };
        use am_light_lib::backends::PlaybackBackend;
        use am_light_lib::smtc::SmtcState;
        use std::sync::Arc;

        // P0-1: O(1) revision snapshot & direct-mutation safety
        let mut ui = AppUi::new(0);
        let rev0 = ui.content_revision_snapshot();
        assert_eq!(rev0, (0, 0, 0, 0));

        // P0-2: active_queue_tracks_cached returns same Arc across calls without 750ms forced churn
        let q1 = ui.active_queue_tracks_cached();
        let q2 = ui.active_queue_tracks_cached();
        assert!(Arc::ptr_eq(&q1, &q2), "unchanged queue cache must reuse exact Arc");

        // P0-4: Non-Home settled playing cadence relaxes to 4 Hz (250ms) when idle with no visible EQ/motion
        ui.settings.playback_backend = PlaybackBackend::AppleMusicWeb;
        ui.settings.live_eq_icon_spectrum = false;
        ui.settings.reduce_motion = false;
        ui.is_window_focused = true;
        ui.route = PageRoute::Settings;
        ui.show_lyrics_overlay = false;
        ui.route_trans = 1.0;
        ui.sidebar_open_spring.snap(1.0);
        ui.top_bar_guard_spring.snap(0.0);
        ui.on_playback_state_changed(SmtcState {
            active: true,
            title: "Idle Settings Track".to_string(),
            artist: "Artist".to_string(),
            status: "Playing".to_string(),
            position_sec: 42.0,
            duration_sec: 200.0,
            backend_mode: "web-am".to_string(),
            adam_id: Some("123456".to_string()),
            position_is_calibrated: true,
            ..Default::default()
        });
        assert_eq!(
            ui.effective_frame_pacing(),
            (0, 4, Some(250_000_000)),
            "settled non-Home Settings page while playing idle must pace at 4 Hz (250ms)"
        );

        // P0-5: Arc-backed SharedPlaylistDetail keeps JSON schema identical and clones in O(1) via Arc
        let mut cache = PlaylistDiskCache::default();
        let detail = CachedPlaylistDetail {
            id: "p.arc1".to_string(),
            kind: "library-playlists".to_string(),
            title: "Shared Playlist".to_string(),
            subtitle: "Owner".to_string(),
            description: String::new(),
            artwork_url: Some("https://is1-ssl.mzstatic.com/a/600x600bb.jpg".to_string()),
            tracks: Vec::new(),
            updated_unix_sec: 1_700_000_000,
        };
        cache
            .details
            .insert("p.arc1".to_string(), SharedPlaylistDetail(Arc::new(detail)));
        let cloned = cache.clone();
        assert!(
            Arc::ptr_eq(&cache.details["p.arc1"].0, &cloned.details["p.arc1"].0),
            "cloning PlaylistDiskCache must share Arc without deep-cloning tracks"
        );
        let json_str = serde_json::to_string(&cache).expect("serialize PlaylistDiskCache");
        let parsed: PlaylistDiskCache =
            serde_json::from_str(&json_str).expect("deserialize PlaylistDiskCache");
        assert_eq!(parsed.details["p.arc1"].title, "Shared Playlist");

        // P0-6: Zero-alloc UTF-8-safe ASCII case-insensitive substring matcher
        assert!(contains_ascii_icase("Jay Chou 周杰伦 - Sunny Day", "周杰伦"));
        assert!(contains_ascii_icase("HELLO World", "hello"));
        assert!(contains_ascii_icase("MixedCase123", "case123"));
        assert!(contains_ascii_icase("anything", ""));
        assert!(!contains_ascii_icase("abc", "abcd"));
        assert!(!contains_ascii_icase("Sunny Day", "night"));

        // P0-8 / P1-2: Tiered mzstatic artwork request sizing without corrupting NetEase/QQ/query URLs
        let mz600 = "https://is1-ssl.mzstatic.com/image/thumb/a/600x600bb.jpg";
        let mz360 = "https://is1-ssl.mzstatic.com/image/thumb/a/360x360bb.jpg";
        assert_eq!(
            upgrade_artwork_request_url_dim(mz600, 160),
            "https://is1-ssl.mzstatic.com/image/thumb/a/240x240bb.jpg"
        );
        assert_eq!(
            upgrade_artwork_request_url_dim(mz360, 384),
            "https://is1-ssl.mzstatic.com/image/thumb/a/480x480bb.jpg"
        );
        assert_eq!(
            upgrade_artwork_request_url_dim(mz360, 720),
            "https://is1-ssl.mzstatic.com/image/thumb/a/720x720bb.jpg"
        );
        let ne_url = "https://p1.music.126.net/hash/600x600_id.jpg?param=480y480";
        let qq_url = "https://y.gtimg.cn/music/photo_new/T002R500x500M000abc.jpg";
        assert_eq!(upgrade_artwork_request_url_dim(ne_url, 160), ne_url);
        assert_eq!(upgrade_artwork_request_url_dim(qq_url, 160), qq_url);

        // Verify direct pub mutation of `home_shelves` still registers cleanly on AppUi
        ui.home_shelves.push(EditorialShelf {
            title: "Direct Shelf".to_string(),
            subtitle: String::new(),
            kind: EditorialShelfKind::SquareShelf,
            items: vec![EditorialItem {
                id: "s1".to_string(),
                kind: "album".to_string(),
                play_kind: "album".to_string(),
                title: "Item 1".to_string(),
                ..Default::default()
            }],
        });
        assert_eq!(ui.home_shelves.len(), 1);
        drop(ui);
    }

    #[test]
    fn test_phase2_perf_gpu_strip_vram_true_lru_caches_and_lyrics_o1() {
        use am_light_lib::d2d_renderer::{
            ArtworkSizeTier, D2DRenderer, DecodedArtworkCpu, SfIcon, TextAlign,
        };
        use am_light_lib::lyrics::{
            get_cached_lyrics, lyrics_cache_len, parse_lrc_lyrics, put_custom_lyrics_in_cache,
            NonEmphasisLyricsEffect,
        };
        use am_light_lib::lyrics_view::LyricsViewState;
        use am_light_lib::smtc::SmtcState;
        use am_light_lib::theme::{Color, RectF};
        use windows::core::{w, PCWSTR};
        use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, CS_HREDRAW, CS_VREDRAW,
            WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
        };

        // P1-2 & P1-4 pure checks
        assert_eq!(D2DRenderer::decode_dim_for_tier(ArtworkSizeTier::SmallThumb, false), 128);
        assert_eq!(D2DRenderer::decode_dim_for_tier(ArtworkSizeTier::Card, false), 384);
        assert_eq!(D2DRenderer::decode_dim_for_tier(ArtworkSizeTier::Hero, false), 720);
        assert!(!D2DRenderer::is_volatile_telemetry_mono_text("3:42"));
        assert!(!D2DRenderer::is_volatile_telemetry_mono_text("1"));
        assert!(!D2DRenderer::is_volatile_telemetry_mono_text("0.0s"));
        assert!(D2DRenderer::is_volatile_telemetry_mono_text("-03:45"));
        assert!(D2DRenderer::is_volatile_telemetry_mono_text("143.8 FPS"));
        assert!(D2DRenderer::is_volatile_telemetry_mono_text("-12.4 dB"));

        // P2-1: O(1) arithmetic queue visible range for 10,000 tracks
        let (q_start, q_end) = LyricsViewState::compute_queue_visible_range(10_000, 6000.0, 480.0);
        assert!(q_start >= 95 && q_start <= 105, "unexpected q_start={q_start}");
        assert!(q_end > q_start && (q_end - q_start) <= 16, "queue slice must stay O(1) viewport-bounded: {q_start}..{q_end}");

        // P2-1: O(L + T) monotonic translation alignment on 200 lines + 200 translations
        let mut lrc_main = String::new();
        let mut lrc_trans = String::new();
        for i in 0..200 {
            let sec = i * 2;
            let mm = sec / 60;
            let ss = sec % 60;
            lrc_main.push_str(&format!("[{mm:02}:{ss:02}.00]Line {i}\n"));
            lrc_trans.push_str(&format!("[{mm:02}:{ss:02}.00]Trans {i}\n"));
        }
        let parsed_200 =
            parse_lrc_lyrics(&lrc_main, Some(&lrc_trans), "QA-LRC").expect("parse 200-line lrc");
        assert_eq!(parsed_200.lines.len(), 200);
        assert_eq!(parsed_200.lines[0].translation.as_deref(), Some("Trans 0"));
        assert_eq!(parsed_200.lines[99].translation.as_deref(), Some("Trans 99"));
        assert_eq!(parsed_200.lines[199].translation.as_deref(), Some("Trans 199"));

        // P2-1: Bounded 48-entry true-LRU lyrics cache (touched early key survives 60 inserts)
        for i in 0..60 {
            if i == 30 {
                let _ = get_cached_lyrics("qa_p2_lru_0");
            }
            put_custom_lyrics_in_cache(&format!("qa_p2_lru_{i}"), parsed_200.clone());
        }
        assert!(lyrics_cache_len() <= 48, "LYRICS_CACHE must stay bounded <= 48");
        assert!(
            get_cached_lyrics("qa_p2_lru_0").is_some(),
            "touched early lyrics cache key must survive LRU eviction"
        );
        assert!(
            get_cached_lyrics("qa_p2_lru_1").is_none(),
            "untouched oldest lyrics cache key must be evicted"
        );

        // Real D2DRenderer checks at wide 2560x1440 resolution
        unsafe extern "system" fn gpu_p2_wnd_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }
        let hwnd = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
            let cls = w!("AMLightPhase2GpuHost");
            let wc = WNDCLASSW {
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(gpu_p2_wnd_proc),
                hInstance: hinst,
                lpszClassName: cls,
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                cls,
                PCWSTR(w!("AM Light Phase2 GPU").as_ptr()),
                WS_POPUP,
                0,
                0,
                2560,
                1440,
                None,
                None,
                Some(hinst),
                None,
            )
            .expect("CreateWindowExW Phase2 GPU")
        };

        let mut r = D2DRenderer::new(hwnd, 2560, 1440, 1.0).expect("D2DRenderer::new 2560x1440");

        // P1-1: Strip-mode liquid glass VRAM allocation must be < 6 MB (vs ~20.4 MB full-window)
        r.begin_draw(Color::rgb(12, 12, 16));
        r.begin_glass_backdrop_snapshot_strips_ex(false, 72.0, 92.0, None, 0b11);
        r.end_glass_backdrop_snapshot();
        r.end_window_draw_and_present(0.0, true, 0);
        let strip_bytes = r.glass_snapshot_allocated_bytes();
        let (_tw, th, _bw, bh) = r.glass_strip_snapshot_dims();
        assert!(
            strip_bytes > 0 && strip_bytes < 6_000_000,
            "strip-mode glass VRAM must stay < 6 MB on 2560x1440, got {strip_bytes}"
        );
        assert!(th > 0 && th < 300 && bh > 0 && bh < 300, "strip heights must be << 1440: th={th}, bh={bh}");

        // P1-2: True LRU recency promotion on draw_rounded_artwork + no-downgrade guard
        let mk_art = |dim: u32, px: u8| DecodedArtworkCpu {
            pixels: vec![px; (dim * dim * 4) as usize],
            width: dim,
            height: dim,
            palette: [Color::rgb(px, 60, 90); 4],
        };
        let url_a = "https://is1-ssl.mzstatic.com/image/thumb/lru_a/480x480bb.jpg";
        let url_b = "https://is1-ssl.mzstatic.com/image/thumb/lru_b/480x480bb.jpg";
        assert!(r.upload_decoded_artwork(url_a.to_string(), mk_art(384, 200), false));
        assert!(r.upload_decoded_artwork(url_b.to_string(), mk_art(384, 100), false));
        // Touch url_a via draw_rounded_artwork -> url_a must become MRU!
        r.begin_draw(Color::rgb(12, 12, 16));
        assert!(r.draw_rounded_artwork(url_a, false, RectF::new(10.0, 10.0, 120.0, 120.0), 8.0, 1.0));
        r.end_window_draw_and_present(0.0, true, 0);
        let recency = r.thumb_cache_keys_by_recency();
        assert_eq!(
            recency.last().map(String::as_str),
            Some("https://is1-ssl.mzstatic.com/image/thumb/lru_a"),
            "draw_rounded_artwork hit must promote stem to MRU (last in ascending recency order): {recency:?}"
        );
        // No-downgrade guard: attempting to upload 128x128 for url_a must NOT downgrade 384px cache!
        let _ = r.upload_decoded_artwork(url_a.to_string(), mk_art(128, 50), false);
        assert!(
            r.has_artwork_min_dim(url_a, false, 384),
            "smaller 128px decode must never downgrade cached 384px artwork"
        );

        // P1-4: Static mono cached vs volatile uncached, radial glow + icon geometry caches, 0.5x transition blur
        r.begin_draw(Color::rgb(12, 12, 16));
        let tl_before = r.text_layout_cache_len();
        r.draw_mono_text(
            "3:42",
            RectF::new(10.0, 10.0, 80.0, 24.0),
            12.0,
            500,
            Color::rgb(255, 255, 255),
            TextAlign::Right,
            true,
        );
        let tl_after_static = r.text_layout_cache_len();
        assert!(tl_after_static > tl_before, "static duration '3:42' must be cached in text_layout_cache");
        r.draw_mono_text_volatile(
            "1:23",
            RectF::new(10.0, 40.0, 80.0, 24.0),
            12.0,
            500,
            Color::rgb(255, 255, 255),
            TextAlign::Left,
            true,
        );
        assert_eq!(
            r.text_layout_cache_len(),
            tl_after_static,
            "draw_mono_text_volatile must bypass text_layout_cache"
        );
        r.fill_radial_glow(100.0, 100.0, 80.0, 80.0, Color::rgba(255, 80, 120, 0.6), Color::rgba(255, 80, 120, 0.0));
        r.draw_icon(SfIcon::Play, RectF::new(200.0, 200.0, 24.0, 24.0), Color::rgb(255, 255, 255));
        assert!(r.radial_glow_cache_len() > 0, "radial glow brush must populate radial_glow_cache");
        assert!(r.icon_geom_cache_len() > 0, "static SfIcon::Play must populate icon_geom_cache");
        if r.begin_lyrics_transition_sheet() {
            r.end_and_composite_lyrics_transition_sheet(0.5, 10.0, 0.0, 1.0, 12.0, false);
        }
        r.end_window_draw_and_present(0.0, true, 0);
        let (tr_w, tr_h) = r.lyrics_transition_blur_dims();
        assert_eq!((tr_w, tr_h), (1280, 720), "lyrics transition blur must run at 0.5x half-resolution");

        // P0-8: Wide-window adaptive lyrics blur scale & 30Hz aurora-gated lyrics glass strip reuse
        r.set_lyrics_blur_downsample_explicit(true, true);
        assert!(r.effective_lyrics_blur_scale() < 0.75);
        r.set_lyrics_blur_downsample_explicit(false, true);
        assert!((r.effective_lyrics_blur_scale() - 1.0).abs() < 1e-4);

        let mut lv = LyricsViewState::default();
        let win_rect = RectF::new(0.0, 0.0, 2560.0, 1440.0);
        let smtc = SmtcState {
            active: true,
            title: "Aurora Test".to_string(),
            artist: "Artist".to_string(),
            status: "Playing".to_string(),
            position_sec: 10.0,
            duration_sec: 180.0,
            ..Default::default()
        };
        // Frame 1 primes aurora + lyrics glass strips
        r.begin_draw(Color::rgb(12, 12, 16));
        lv.draw(
            &mut r,
            win_rect,
            &smtc,
            Some(url_a),
            Some(&parsed_200),
            "qa_p2_aurora",
            false,
            false,
            0.0,
            0.2,
            false,
            false,
            (-10.0, -10.0),
            false,
            false,
            false,
            false,
            false,
            true,
            NonEmphasisLyricsEffect::Both,
            true,
            true,
            true,
            1.0,
            None,
        );
        r.end_window_draw_and_present(0.0, true, 0);

        // Frame 2 immediately after (within 33ms aurora gate) -> 0 glass flushes, 0 glass blurs, >=1 cache hit!
        r.reset_frame_stats();
        r.begin_draw(Color::rgb(12, 12, 16));
        lv.draw(
            &mut r,
            win_rect,
            &smtc,
            Some(url_a),
            Some(&parsed_200),
            "qa_p2_aurora",
            false,
            false,
            0.0,
            0.2,
            false,
            false,
            (-10.0, -10.0),
            false,
            false,
            false,
            false,
            false,
            true,
            NonEmphasisLyricsEffect::Both,
            true,
            true,
            true,
            1.0,
            None,
        );
        r.end_window_draw_and_present(0.0, true, 0);
        let st_lyr = r.frame_stats();
        assert_eq!(st_lyr.glass_snapshot_flushes, 0, "non-aurora lyrics frame must incur 0 glass flushes");
        assert_eq!(st_lyr.glass_snapshot_blurs, 0, "non-aurora lyrics frame must incur 0 glass blurs");
        assert!(st_lyr.glass_snapshot_cache_hits >= 1, "non-aurora lyrics frame must reuse cached glass strip");

        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    #[test]
    fn test_phase2_perf_motion_cancel_lru_cadence_and_logger() {
        use am_light_lib::motion_art::{
            decode_tick_stride_for_slot, parse_avc_variant_for_target_dim, MotionArtManager,
            MotionDiskCacheLru, CORE_VIEWPORT_FULL_CADENCE_SLOTS, MAX_DECODER_OPENS_PER_TICK,
            MOTION_CACHE_MAX_BYTES, MOTION_CACHE_MAX_FILES, MOTION_CACHE_TARGET_BYTES,
            MOTION_CACHE_TARGET_FILES,
        };

        assert_eq!(MOTION_CACHE_MAX_FILES, 128);
        assert_eq!(MOTION_CACHE_TARGET_FILES, 96);
        assert_eq!(MOTION_CACHE_MAX_BYTES, 256 * 1024 * 1024);
        assert_eq!(MOTION_CACHE_TARGET_BYTES, 200 * 1024 * 1024);
        assert!(CORE_VIEWPORT_FULL_CADENCE_SLOTS >= 8);
        assert!(MAX_DECODER_OPENS_PER_TICK <= 4);

        // P0-3: Cancellable HLS downloads via AbortHandle
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt");
        let mgr = MotionArtManager::new();
        let cancel_url = "https://example.invalid/phase2_cancel_me.m3u8";
        let jh = rt.spawn(async {
            std::future::pending::<()>().await;
        });
        mgr.register_inflight_abort_handle_for_url(cancel_url, jh.abort_handle());
        assert_eq!(mgr.inflight_download_count(), 1);
        mgr.sync_prefetch_urls(&[]);
        assert_eq!(mgr.inflight_download_count(), 0, "leaving viewport+prefetch must abort inflight download");
        assert!(mgr.aborted_download_count() >= 1);
        rt.block_on(async {
            let res = jh.await;
            assert!(res.is_err() && res.unwrap_err().is_cancelled(), "aborted download task must be cancelled");
        });

        // P0-3: Non-blocking readiness returns false immediately for uncached URL (0 disk stats)
        assert!(!mgr.is_url_cached_ready_nonblocking("https://example.invalid/definitely_absent_p2.m3u8"));
        mgr.set_global_enabled(false);

        // P0-3: MotionDiskCacheLru hysteresis (max 6 -> target 4 files; max 10KB -> target 7KB) + true LRU touch
        let tmp_dir = std::env::temp_dir().join("am_light_p2_motion_lru_test");
        let _ = std::fs::remove_dir_all(&tmp_dir);
        let _ = std::fs::create_dir_all(&tmp_dir);
        let mut lru = MotionDiskCacheLru::new(tmp_dir.clone(), 6, 4, 10_000, 7_000);
        for i in 0..6 {
            let ev = lru.record_write(&format!("k{i}"), 1_000);
            assert!(ev.is_empty());
        }
        assert!(lru.touch("k0").is_some(), "touching k0 must promote it to MRU");
        let evicted = lru.record_write("k6", 1_000);
        assert_eq!(evicted.len(), 3, "exceeding 6 files must evict down to hysteresis target 4");
        assert_eq!(lru.len(), 4);
        assert!(lru.contains_key("k0"), "touched k0 must survive LRU eviction");
        assert!(!lru.contains_key("k1"), "untouched oldest k1 must be evicted");
        // Byte-cap hysteresis: push total_bytes past 10_000 -> evict down to <= 7_000
        let ev_bytes = lru.record_write("k_big", 6_500);
        assert!(!ev_bytes.is_empty());
        assert!(lru.total_bytes() <= 7_000, "byte hysteresis must prune down to <= 7000, got {}", lru.total_bytes());
        let _ = std::fs::remove_dir_all(&tmp_dir);

        // P1-1: Avoid 960p jump on bucket 512 while preserving 720p selection when present
        let m3u8_jump = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1200000,CODECS=\"avc1.64001f\",RESOLUTION=486x486\n\
            486.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=4500000,CODECS=\"avc1.640028\",RESOLUTION=960x960\n\
            960.m3u8\n";
        let picked_486 = parse_avc_variant_for_target_dim("https://cdn.example/master.m3u8", m3u8_jump, 512)
            .expect("pick 486 over 960");
        assert!(picked_486.ends_with("/486.m3u8"), "must pick 486 instead of jumping to 960p: {picked_486}");
        let m3u8_mid = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1200000,CODECS=\"avc1.64001f\",RESOLUTION=486x486\n\
            486.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=2600000,CODECS=\"avc1.640020\",RESOLUTION=720x720\n\
            720.m3u8\n";
        let picked_720 = parse_avc_variant_for_target_dim("https://cdn.example/master.m3u8", m3u8_mid, 512)
            .expect("pick 720 when available");
        assert!(picked_720.ends_with("/720.m3u8"), "must still pick 720p when intermediate variant exists: {picked_720}");

        // P1-1: Tiered viewport cadence
        assert_eq!(decode_tick_stride_for_slot(0, 20), 1);
        assert_eq!(decode_tick_stride_for_slot(CORE_VIEWPORT_FULL_CADENCE_SLOTS - 1, 20), 1);
        assert_eq!(decode_tick_stride_for_slot(CORE_VIEWPORT_FULL_CADENCE_SLOTS, 20), 2);

        // P2-3: Persistent buffered logger writer
        am_light_lib::logger::init();
        am_light_lib::logger::info("qa", "phase2 perf logger persistence check");
        am_light_lib::logger::flush();
        assert!(
            am_light_lib::logger::is_persistent_writer_open(),
            "logger must maintain persistent BufWriter<File> across writes"
        );
    }

    #[test]
    fn test_phase2_perf_network_audio_dsp_and_aux_storage() {
        use am_light_lib::am_agent::{
            BoundedLruMap, AGENT_LYRICS_CACHE_MAX_ENTRIES, HLS_MANIFEST_CACHE_MAX_ENTRIES,
            TRACK_META_CACHE_MAX_ENTRIES, TRACK_TRAITS_CACHE_MAX_ENTRIES,
        };
        use am_light_lib::am_api::{
            artwork_canonical_stem, artwork_canonical_stem_ref, fetch_artwork_bytes_if_current,
            ordered_motion_probe_storefronts, prune_art_cache_dir, write_artwork_cache_file_atomic,
            CachedDevToken, DevTokenSingleflight,
        };
        use am_light_lib::audio_viz::{
            compute_unbiased_autocorr_fft, sample_live_visualizer, set_active_viz_style_inspector,
            SharedSpectrumAnalyzer,
        };
        use am_light_lib::backends::{ResolvedAudioStream, WebAmStatus};
        use am_light_lib::binaural_dsp::{
            legacy_restore_marker_path_in_base, pop_restore_marker_in_base,
            restore_marker_path_in_base, save_restore_marker_in_base,
        };
        use am_light_lib::drm_fix::{prune_old_drm_backups, MAX_DRM_BACKUPS};
        use am_light_lib::listen_together::{
            compute_next_poll_interval_ms, AdaptivePollState, ListenTogetherStatus,
            POLL_INTERVAL_ACTIVE_MS, POLL_INTERVAL_IDLE_MAX_MS, POLL_INTERVAL_IDLE_STEP1_MS,
        };
        use am_light_lib::native_player::{
            find_cached_stream_file, inspect_audio_file_specs_from_header,
            is_download_cancelled_err, stable_stream_cache_key, stable_stream_cache_key_from_url,
            strip_ephemeral_url_query,
        };
        use am_light_lib::visualizers::VisualizerState;
        use am_light_lib::web_am_player::{build_edge_launch_args, status_eq};
        use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
        use std::sync::Arc;

        // P1-2 & P1-5: am_api stem_ref, epoch cancel, atomic write + prune, DevTokenSingleflight, storefront ordering
        let sample_art = "https://is1-ssl.mzstatic.com/image/thumb/Music/v4/11/22/33/600x600bb.jpg";
        assert_eq!(artwork_canonical_stem_ref(sample_art), artwork_canonical_stem(sample_art));
        assert_eq!(ordered_motion_probe_storefronts("cn"), vec!["cn", "us"]);
        assert_eq!(ordered_motion_probe_storefronts("us"), vec!["us"]);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio rt");
        rt.block_on(async {
            let epoch = AtomicU64::new(5);
            let res = fetch_artwork_bytes_if_current(
                "https://example.invalid/stale.jpg",
                false,
                Some((&epoch, 4)),
            )
            .await;
            assert_eq!(res.unwrap_err(), "CANCELLED");

            // DevTokenSingleflight: 5 concurrent cold-start callers trigger only 1 fetch
            let sf = Arc::new(DevTokenSingleflight::new());
            let fetch_calls = Arc::new(AtomicUsize::new(0));
            let mut tasks = Vec::new();
            for _ in 0..5 {
                let sf_c = Arc::clone(&sf);
                let fc = Arc::clone(&fetch_calls);
                tasks.push(tokio::spawn(async move {
                    sf_c.get_or_fetch(false, 1_000, move || {
                        let fc = Arc::clone(&fc);
                        async move {
                            fc.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                            Ok(CachedDevToken {
                                token: "singleflight_jwt".to_string(),
                                kid: Some("kid1".to_string()),
                                exp: Some(9_999_999),
                            })
                        }
                    })
                    .await
                }));
            }
            for t in tasks {
                let tok = t.await.unwrap().unwrap();
                assert_eq!(tok.token, "singleflight_jwt");
            }
            assert_eq!(fetch_calls.load(Ordering::SeqCst), 1, "DevTokenSingleflight must coalesce 5 concurrent callers into 1 fetch");
        });

        let art_tmp = std::env::temp_dir().join("am_light_p2_art_cache_test");
        let _ = std::fs::remove_dir_all(&art_tmp);
        let _ = std::fs::create_dir_all(&art_tmp);
        for i in 0..6 {
            let p = art_tmp.join(format!("{i:016x}.img"));
            write_artwork_cache_file_atomic(&p, &[i as u8; 512]).expect("atomic write");
            std::thread::sleep(std::time::Duration::from_millis(12));
        }
        let pruned = prune_art_cache_dir(&art_tmp, 4, 2, 100_000, 100_000);
        assert_eq!(pruned, 4, "art_cache prune must evict 6 -> 2 files");
        let _ = std::fs::remove_dir_all(&art_tmp);

        // P1-6: web_am_player Edge cache bounds + status_eq; listen_together adaptive backoff
        let edge_args = build_edge_launch_args(
            std::path::Path::new("C:\\Temp\\prof"),
            9222,
            "https://music.apple.com",
            false,
        );
        assert!(edge_args.iter().any(|a| a == "--disk-cache-size=104857600"));
        assert!(edge_args.iter().any(|a| a == "--media-cache-size=104857600"));
        let ws1 = WebAmStatus::default();
        let ws2 = WebAmStatus::default();
        assert!(status_eq(&ws1, &ws2));

        assert_eq!(POLL_INTERVAL_ACTIVE_MS, 750);
        assert_eq!(POLL_INTERVAL_IDLE_STEP1_MS, 2000);
        assert_eq!(POLL_INTERVAL_IDLE_MAX_MS, 3000);
        assert_eq!(compute_next_poll_interval_ms(true, false, false, 10), 750);
        assert_eq!(compute_next_poll_interval_ms(false, false, false, 1), 2000);
        assert_eq!(compute_next_poll_interval_ms(false, false, false, 4), 3000);
        let mut ap = AdaptivePollState::new();
        let mut lt_st = ListenTogetherStatus {
            remote_song_id: Some("1001".to_string()),
            remote_play_status: "PAUSE".to_string(),
            remote_client_seq: 1,
            ..Default::default()
        };
        assert_eq!(ap.next_delay_ms(false, Some(&lt_st)), 750); // initial state_changed
        assert_eq!(ap.next_delay_ms(false, Some(&lt_st)), 2000); // idle tick 1
        assert_eq!(ap.next_delay_ms(false, Some(&lt_st)), 2000); // idle tick 2
        assert_eq!(ap.next_delay_ms(false, Some(&lt_st)), 3000); // idle tick 3 -> max backoff
        lt_st.remote_song_id = Some("1002".to_string());
        assert_eq!(ap.next_delay_ms(false, Some(&lt_st)), 750); // instant snap back on track change

        // P2-1: am_agent::BoundedLruMap
        assert_eq!(TRACK_META_CACHE_MAX_ENTRIES, 4096);
        assert_eq!(TRACK_TRAITS_CACHE_MAX_ENTRIES, 4096);
        assert_eq!(HLS_MANIFEST_CACHE_MAX_ENTRIES, 2048);
        assert_eq!(AGENT_LYRICS_CACHE_MAX_ENTRIES, 32);
        let mut blru: BoundedLruMap<u32> = BoundedLruMap::new(3);
        blru.insert("k0".to_string(), 10);
        blru.insert("k1".to_string(), 20);
        blru.insert("k2".to_string(), 30);
        assert_eq!(blru.get_cloned_touch("k0"), Some(10));
        blru.insert("k3".to_string(), 40);
        assert_eq!(blru.len(), 3);
        assert!(blru.contains_key("k0"), "touched k0 must survive BoundedLruMap eviction");
        assert!(!blru.contains_key("k1"), "untouched oldest k1 must be evicted");

        // P0-7: native_player stable stream cache key, rotating token strip, multi-ext probe & 64-byte FLAC header
        let ne_u1 = "https://m801.music.126.net/20261128010203/0123456789abcdef0123456789abcdef/ymusic/obj/song123.flac?auth=aaa";
        let ne_u2 = "https://m701.music.126.net/20261128095959/fedcba9876543210fedcba9876543210/ymusic/obj/song123.flac?auth=bbb";
        assert_eq!(strip_ephemeral_url_query(ne_u1), strip_ephemeral_url_query(ne_u2));
        assert_eq!(
            stable_stream_cache_key_from_url(ne_u1, "FLAC", false),
            stable_stream_cache_key_from_url(ne_u2, "FLAC", false)
        );
        let skey = stable_stream_cache_key("netease", "ne:98765", "hires");
        assert_eq!(skey, stable_stream_cache_key("NetEase", "98765", "HIRES"));
        assert!(is_download_cancelled_err("cancelled: superseded"));
        assert!(!is_download_cancelled_err("HTTP 404"));

        let st_tmp = std::env::temp_dir().join("am_light_p2_stream_cache_test");
        let _ = std::fs::remove_dir_all(&st_tmp);
        let _ = std::fs::create_dir_all(&st_tmp);
        let cached_flac = st_tmp.join(format!("trk_{skey:016x}.flac"));
        std::fs::write(&cached_flac, vec![0x66u8; 16_384]).expect("write cached flac");
        assert_eq!(find_cached_stream_file(&st_tmp, skey), Some(cached_flac));
        let _ = std::fs::remove_dir_all(&st_tmp);

        // Synthetic 64-byte FLAC STREAMINFO header: 48,000 Hz, 2ch, 24-bit
        let mut flac_head = vec![0u8; 64];
        flac_head[0..4].copy_from_slice(b"fLaC");
        flac_head[4] = 0x80; // last metadata block, type 0 (STREAMINFO)
        flac_head[5..8].copy_from_slice(&[0, 0, 34]); // length 34
        // Bytes 18..22 pack: sample_rate (20 bits) | (channels-1) (3 bits) | (bps-1) (5 bits) | total_samples_hi (4 bits)
        let packed: u32 = (48_000u32 << 12) | (1u32 << 9) | (23u32 << 4);
        flac_head[18..22].copy_from_slice(&packed.to_be_bytes());
        let dummy_stream = ResolvedAudioStream {
            url: ne_u1.to_string(),
            referer: String::new(),
            cookie: None,
            file_ext: "flac".to_string(),
            quality_label: "FLAC".to_string(),
            nominal_bitrate_kbps: 1411,
            is_trial: false,
        };
        let specs = inspect_audio_file_specs_from_header(40_000_000, &flac_head, &dummy_stream, 44_100, 2, 240.0);
        assert_eq!(specs.codec, "FLAC");
        assert_eq!(specs.sample_rate_hz, 48_000);
        assert_eq!(specs.bit_depth, Some(24));

        // P1-3: Exact O(M log M) unbiased autocorr parity vs naive O(M^2), SharedSpectrumAnalyzer, Inspector DSP gate & Visualizer draw-call caps
        let sig: Vec<f32> = (0..192)
            .map(|i| (2.0 * PI * (i as f32) / 16.0).sin() + 0.35 * (2.0 * PI * (i as f32) / 7.0).cos())
            .collect();
        let fast_ac = compute_unbiased_autocorr_fft(&sig, 20, 60);
        assert_eq!(fast_ac.len(), 62);
        for lag in 10..=60 {
            let mut dot = 0.0f32;
            let mut e1 = 0.0f32;
            let mut e2 = 0.0f32;
            for i in 0..(sig.len() - lag) {
                dot += sig[i] * sig[i + lag];
                e1 += sig[i] * sig[i];
                e2 += sig[i + lag] * sig[i + lag];
            }
            let naive = if e1 > 1e-6 && e2 > 1e-6 {
                (dot / (e1 * e2).sqrt()).clamp(-1.0, 1.0)
            } else {
                0.0
            };
            assert!(
                (fast_ac[lag] - naive).abs() < 1e-4,
                "FFT autocorr mismatch at lag={lag}: fast={} naive={naive}",
                fast_ac[lag]
            );
        }

        let mut analyzer = SharedSpectrumAnalyzer::new();
        let sr = 48_000u32;
        let sine_2048: Vec<f32> = (0..2048)
            .map(|n| 0.85 * (2.0 * PI * 1000.0 * (n as f32) / (sr as f32)).sin())
            .collect();
        let (b32, b64, b120) = analyzer.compute_raw_bands(&sine_2048, sr);
        assert!(b32.iter().copied().fold(0.0f32, f32::max) > 0.5);
        assert!(b64.iter().copied().fold(0.0f32, f32::max) > 0.5);
        assert!(b120.iter().copied().fold(0.0f32, f32::max) > 0.5);
        let fast32 = analyzer.compute_raw_32_fast(&sine_2048[..512], sr);
        assert!(fast32.iter().copied().fold(0.0f32, f32::max) > 0.5);

        set_active_viz_style_inspector(false);
        let gated_snap = sample_live_visualizer(0.016);
        assert!(gated_snap.soundstage_lr.is_empty());
        assert!(gated_snap.soundstage_band_pts.iter().all(|v| v.is_empty()));
        set_active_viz_style_inspector(true);

        let viz = VisualizerState::default();
        assert_eq!(viz.matrix_pillar_count(), 1369);
        assert!(
            viz.matrix_render_active_count() <= 361,
            "stride-2 ChernobylRods render count must stay <= 361, got {}",
            viz.matrix_render_active_count()
        );
        assert!(viz.soundstage_stats().0 <= 420);

        // P1-6: Binaural marker migration & DRM backup pruning (keep latest 3)
        let bin_tmp = std::env::temp_dir().join("am_light_p2_binaural_marker_test");
        let _ = std::fs::remove_dir_all(&bin_tmp);
        let legacy_path = legacy_restore_marker_path_in_base(&bin_tmp);
        let primary_path = restore_marker_path_in_base(&bin_tmp);
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        std::fs::write(&legacy_path, "legacy-dev-1").unwrap();
        // `pop_restore_marker_in_base` migrates legacy-only marker and removes it
        assert_eq!(pop_restore_marker_in_base(&bin_tmp).as_deref(), Some("legacy-dev-1"));
        assert!(!legacy_path.exists());
        // `save_restore_marker_in_base` writes under AMLight\ and removes legacy file
        std::fs::create_dir_all(legacy_path.parent().unwrap()).unwrap();
        std::fs::write(&legacy_path, "stale-legacy").unwrap();
        save_restore_marker_in_base(&bin_tmp, "new-dev-2");
        assert!(primary_path.exists());
        assert!(!legacy_path.exists());
        assert_eq!(pop_restore_marker_in_base(&bin_tmp).as_deref(), Some("new-dev-2"));
        let _ = std::fs::remove_dir_all(&bin_tmp);

        let drm_tmp = std::env::temp_dir().join("am_light_p2_drm_prune_test");
        let _ = std::fs::remove_dir_all(&drm_tmp);
        let _ = std::fs::create_dir_all(&drm_tmp);
        assert_eq!(MAX_DRM_BACKUPS, 3);
        for i in 1..=5 {
            std::fs::create_dir_all(drm_tmp.join(format!("amfix-backup-2026112{i}-000000"))).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        std::fs::create_dir_all(drm_tmp.join("unrelated-dir")).unwrap();
        let removed_drm = prune_old_drm_backups(&drm_tmp, MAX_DRM_BACKUPS);
        assert_eq!(removed_drm, 2);
        assert!(drm_tmp.join("unrelated-dir").exists());
        assert!(!drm_tmp.join("amfix-backup-20261121-000000").exists());
        assert!(drm_tmp.join("amfix-backup-20261125-000000").exists());
        let _ = std::fs::remove_dir_all(&drm_tmp);
    }
}
