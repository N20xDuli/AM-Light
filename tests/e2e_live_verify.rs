#[cfg(target_os = "windows")]
#[test]
#[ignore = "Live AMPLibraryAgent + WASAPI integration test; run with --ignored --nocapture"]
fn e2e_verify_login_persistence_and_live_playback() {
    use am_light_lib::{am_agent, am_api, app_ui::AppSettings, audio_viz};
    use std::time::Duration;

    // 1. Initialize process COM security first (same order as AM-Light.exe startup)
    let sec_hr = am_agent::init_process_com_security();
    println!("[E2E] init_process_com_security hr = 0x{:08X}", sec_hr as u32);

    // 2. Verify automatic reading of official Apple Music desktop client login & legacy token migration
    let (real_settings, official) = AppSettings::load_with_official();
    println!(
        "[E2E] Official client: signed_in={}, dsid={:?}, storefront={:?}, sub_active={}, display_name={:?}, lib_name={:?}",
        official.signed_in,
        official.dsid,
        official.storefront,
        official.subscription_active,
        official.display_name,
        official.library_name
    );
    println!(
        "[E2E] Loaded settings: storefront={}, user_token_len={}, masked={}",
        real_settings.storefront,
        real_settings.user_token.len(),
        am_api::mask_user_token_preview(&real_settings.user_token)
    );
    assert!(official.signed_in, "Official Apple Music client should be detected as signed in");
    assert_eq!(official.storefront.as_deref(), Some("jp"));

    // 3. Verify Token paste -> save -> close -> reopen persistence round-trip
    let test_tok = "0.Ak/TestPersistenceRoundTripTokenValue_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+/=";
    let mut temp_settings = real_settings.clone();
    temp_settings.user_token = am_api::normalize_user_token(test_tok);
    temp_settings.token_cleared_by_user = false;
    temp_settings.save();

    let reloaded = AppSettings::load();
    assert_eq!(
        reloaded.user_token, test_tok,
        "Saved Media-User-Token must persist across reload!"
    );

    // Restore user's real settings immediately and verify restoration
    real_settings.save();
    let restored = AppSettings::load();
    assert_eq!(restored.user_token, real_settings.user_token);
    println!("[E2E] Token persistence round-trip & restore PASSED!");

    // 4. Verify live Apple Music API auth & catalog fetch + REAL AMPLibraryAgent playback!
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio rt");

    let tracks = rt.block_on(async {
        if !real_settings.user_token.is_empty() {
            let auth = am_api::verify_am_auth(
                Some(real_settings.user_token.clone()),
                Some(real_settings.storefront.clone()),
            )
            .await
            .expect("verify_am_auth");
            println!(
                "[E2E] verify_am_auth: dev_ok={}, user_ok={}, detected_sf={:?}, msg={}",
                auth.dev_token_ok, auth.user_token_ok, auth.detected_storefront, auth.message
            );
        }

        let sf = &real_settings.storefront;
        let chart_path = format!(
            "v1/catalog/{sf}/charts?types=songs&extend=audioTraits&limit=5&l=zh-Hans-CN"
        );
        let json = am_api::fetch_am_api(
            chart_path,
            (!real_settings.user_token.is_empty()).then(|| real_settings.user_token.clone()),
            None,
            Some(sf.clone()),
        )
        .await
        .expect("fetch charts");

        let arr = json
            .pointer("/results/songs/0/data")
            .and_then(|v| v.as_array())
            .expect("songs array");
        let parsed: Vec<am_agent::AgentTrackInput> = arr
            .iter()
            .filter_map(|v| am_agent::parse_track_from_am_json(v, None, None))
            .collect();
        assert!(!parsed.is_empty(), "Should parse at least 1 catalog song");
        println!(
            "[E2E] First chart track ({}): id={}, name={} - {}",
            sf, parsed[0].id, parsed[0].name, parsed[0].artist_name
        );
        parsed
    });

    // Subscribe audio visualizer so WASAPI loopback measures live audio output!
    audio_viz::set_viz_subscribed(true);
    let _ = am_agent::control("volume", Some(0.22));
    let play_state = am_agent::play_tracks(tracks, 0).expect("AMPLibraryAgent play_tracks MUST succeed!");
    println!(
        "[E2E] play_tracks initial state: active={}, status={}, title={}, agent_pid={:?}",
        play_state.active, play_state.status, play_state.title, play_state.agent_pid
    );
    assert!(play_state.active);
    assert_eq!(play_state.status, "Playing");

    let _ = am_agent::control("volume", Some(0.22));

    let mut max_pos = 0.0f64;
    let mut best_rms = -120.0f32;
    for i in 0..10 {
        std::thread::sleep(Duration::from_millis(450));
        let st = am_agent::get_playback_state();
        let vf = audio_viz::get_latest_viz_frame();
        if st.position_sec > max_pos {
            max_pos = st.position_sec;
        }
        if vf.rms_db > best_rms {
            best_rms = vf.rms_db;
        }
        println!(
            "[E2E] tick #{}: status={}, pos={:.2}s/{:.1}s, pb_events={}, wasapi_active={}, rms={:.1} dBFS, quality={:?}",
            i + 1,
            st.status,
            st.position_sec,
            st.duration_sec,
            am_agent::playback_event_count(),
            vf.active_signal,
            vf.rms_db,
            st.audio_quality
        );
        if max_pos >= 1.2 && (vf.active_signal || best_rms > -65.0) {
            break;
        }
    }

    let (hr_pb, hr_pbcb, reg_toks) = am_agent::playback_reg_diagnostics();
    println!(
        "[E2E] reg diagnostics: hr_pb=0x{:08X}, hr_pbcb=0x{:08X}, toks=0x{:016X}",
        hr_pb as u32, hr_pbcb as u32, reg_toks
    );

    audio_viz::set_viz_subscribed(false);
    let _ = am_agent::disconnect_agent();

    assert!(
        max_pos > 0.5,
        "Playback position must advance past 0.5s (got {max_pos})"
    );
    assert!(
        best_rms > -75.0 || am_agent::playback_event_count() > 0,
        "Expected real WASAPI audio signal or playback events (best_rms={best_rms})"
    );
    println!("[E2E] ALL LIVE PLAYBACK, OFFICIAL AUTH & PERSISTENCE CHECKS PASSED!");
}
