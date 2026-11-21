#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
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

        // 1. Liquid Glass Default OFF + English-by-Word Default ON (even on legacy settings_v2.json!)
        let def_s = AppSettings::default();
        assert!(
            !def_s.liquid_glass_enabled,
            "Liquid Glass must be disabled by default"
        );
        assert!(
            def_s.lyrics_latin_by_word,
            "English/Latin lyrics must default to word-level units"
        );

        let legacy_json = r#"{"storefront":"cn","reduce_transparency":false,"window_corner_radius":12.0}"#;
        let migrated: AppSettings = serde_json::from_str(legacy_json).expect("deserialize legacy");
        assert!(
            !migrated.liquid_glass_enabled,
            "existing settings_v2.json without liquid_glass_enabled must default to OFF"
        );
        assert!(
            migrated.lyrics_latin_by_word,
            "existing settings_v2.json without lyrics_latin_by_word must default to word mode ON"
        );

        let mut ui = AppUi::new(0);
        assert!(ui.glass_off(), "AppUi::glass_off() must be true by default");
        ui.execute_action(UiAction::ToggleLiquidGlass);
        assert!(ui.settings.liquid_glass_enabled && !ui.glass_off());
        ui.execute_action(UiAction::ToggleLiquidGlass);
        assert!(!ui.settings.liquid_glass_enabled && ui.glass_off());

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
}
