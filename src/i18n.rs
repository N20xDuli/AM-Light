use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum AppLanguage {
    #[default]
    Auto,
    ZhCn,
    ZhTw,
    En,
    Ja,
    Ko,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResolvedLang {
    ZhCn = 0,
    ZhTw = 1,
    En = 2,
    Ja = 3,
    Ko = 4,
}

impl AppLanguage {
    pub const ALL: [AppLanguage; 6] = [
        AppLanguage::Auto,
        AppLanguage::ZhCn,
        AppLanguage::ZhTw,
        AppLanguage::En,
        AppLanguage::Ja,
        AppLanguage::Ko,
    ];

    /// Native autonym displayed on the language selector pill so users of any locale can
    /// always recognize and switch back to their own language immediately.
    pub fn autonym(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::ZhCn => "简体中文",
            Self::ZhTw => "繁體中文",
            Self::En => "English",
            Self::Ja => "日本語",
            Self::Ko => "한국어",
        }
    }

    pub fn resolve(self) -> ResolvedLang {
        match self {
            Self::Auto => detect_system_language(),
            Self::ZhCn => ResolvedLang::ZhCn,
            Self::ZhTw => ResolvedLang::ZhTw,
            Self::En => ResolvedLang::En,
            Self::Ja => ResolvedLang::Ja,
            Self::Ko => ResolvedLang::Ko,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::ZhCn => 1,
            Self::ZhTw => 2,
            Self::En => 3,
            Self::Ja => 4,
            Self::Ko => 5,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::ZhCn,
            2 => Self::ZhTw,
            3 => Self::En,
            4 => Self::Ja,
            5 => Self::Ko,
            _ => Self::Auto,
        }
    }
}

impl ResolvedLang {
    pub fn am_api_locale(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-Hans-CN",
            Self::ZhTw => "zh-Hant-TW",
            Self::En => "en-US",
            Self::Ja => "ja-JP",
            Self::Ko => "ko-KR",
        }
    }

    fn idx(self) -> Option<usize> {
        match self {
            Self::ZhCn => None,
            Self::ZhTw => Some(0),
            Self::En => Some(1),
            Self::Ja => Some(2),
            Self::Ko => Some(3),
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::ZhTw,
            2 => Self::En,
            3 => Self::Ja,
            4 => Self::Ko,
            _ => Self::ZhCn,
        }
    }
}

#[cfg(target_os = "windows")]
extern "system" {
    fn GetUserDefaultUILanguage() -> u16;
}

pub fn detect_system_language() -> ResolvedLang {
    #[cfg(target_os = "windows")]
    {
        let lang_id = unsafe { GetUserDefaultUILanguage() };
        map_win32_langid(lang_id)
    }
    #[cfg(not(target_os = "windows"))]
    {
        ResolvedLang::En
    }
}

/// Maps a Win32 `LANGID` returned by `GetUserDefaultUILanguage()` to a supported `ResolvedLang`.
pub fn map_win32_langid(lang_id: u16) -> ResolvedLang {
    let primary = lang_id & 0x03FF;
    let sub = (lang_id >> 10) & 0x003F;
    match primary {
        0x04 => {
            // Chinese: sublang 0x01 (TW), 0x03 (HK), 0x05 (MO) -> Traditional; 0x02 (CN), 0x04 (SG) -> Simplified
            match sub {
                0x01 | 0x03 | 0x05 => ResolvedLang::ZhTw,
                _ => ResolvedLang::ZhCn,
            }
        }
        0x11 => ResolvedLang::Ja,
        0x12 => ResolvedLang::Ko,
        _ => ResolvedLang::En,
    }
}

static ACTIVE_APP_LANG: AtomicU8 = AtomicU8::new(0); // AppLanguage::Auto
static ACTIVE_RESOLVED_LANG: AtomicU8 = AtomicU8::new(255); // 255 = uninitialized

pub fn set_active_language(lang: AppLanguage) {
    ACTIVE_APP_LANG.store(lang.to_u8(), Ordering::Release);
    ACTIVE_RESOLVED_LANG.store(lang.resolve() as u8, Ordering::Release);
}

#[inline]
pub fn active_language() -> AppLanguage {
    AppLanguage::from_u8(ACTIVE_APP_LANG.load(Ordering::Acquire))
}

#[inline]
pub fn active_resolved_language() -> ResolvedLang {
    let v = ACTIVE_RESOLVED_LANG.load(Ordering::Acquire);
    if v == 255 {
        let resolved = AppLanguage::Auto.resolve();
        ACTIVE_RESOLVED_LANG.store(resolved as u8, Ordering::Release);
        resolved
    } else {
        ResolvedLang::from_u8(v)
    }
}

pub fn am_api_locale() -> &'static str {
    active_resolved_language().am_api_locale()
}

/// Exact UI dictionary: Simplified Chinese key -> `[Traditional Chinese, English, Japanese, Korean]`.
static EXACT_TABLE: Lazy<HashMap<&'static str, [&'static str; 4]>> = Lazy::new(|| {
    let entries: &[(&'static str, [&'static str; 4])] = &[
        // --- Sidebar & Navigation ---
        ("主页", ["首頁", "Home", "ホーム", "홈"]),
        ("新发现", ["新發現", "New", "見つける", "새로운 음악"]),
        ("广播", ["廣播", "Radio", "ラジオ", "라디오"]),
        ("可视化", ["視覺化", "Visualizer", "ビジュアライザ", "시각화"]),
        ("搜索", ["搜尋", "Search", "検索", "검색"]),
        ("资料库", ["資料庫", "Library", "ライブラリ", "보관함"]),
        ("最近添加", ["最近加入", "Recently Added", "最近追加した項目", "최근 추가된 항목"]),
        ("艺人", ["藝人", "Artists", "アーティスト", "아티스트"]),
        ("专辑", ["專輯", "Albums", "アルバム", "앨범"]),
        ("歌曲", ["歌曲", "Songs", "曲", "노래"]),
        ("专属推荐", ["專屬推薦", "Made For You", "あなたにおすすめ", "맞춤 추천"]),
        ("已下载", ["已下載", "Downloaded", "ダウンロード済み", "다운로드됨"]),
        ("播放列表", ["播放清單", "Playlists", "プレイリスト", "플레이리스트"]),
        ("所有播放列表", ["所有播放清單", "All Playlists", "すべてのプレイリスト", "모든 플레이리스트"]),
        ("设置", ["設定", "Settings", "設定", "설정"]),
        ("编辑", ["編輯", "Edit", "編集", "편집"]),
        ("完成", ["完成", "Done", "完了", "완료"]),
        ("取消", ["取消", "Cancel", "キャンセル", "취소"]),
        ("收起", ["收起", "Hide", "閉じる", "접기"]),
        ("返回网格", ["返回網格", "Back to Grid", "グリッドへ戻る", "그리드로 돌아가기"]),

        // --- Backend Headings & Labels ---
        ("播放后端与音乐源", ["播放後端與音樂源", "Playback Backend & Music Source", "再生バックエンドと音源", "재생 백엔드 및 음원"]),
        ("界面语言", ["介面語言", "Language", "言語", "언어"]),
        ("跟随系统自动识别语言，或手动切换界面显示语言（立即生效）", [
            "跟隨系統自動識別語言，或手動切換介面顯示語言（立即生效）",
            "Follow OS locale automatically or switch UI language instantly without restarting",
            "OSの言語に自動追従、または再起動なしで表示言語を即時切替",
            "시스템 언어를 자동으로 따르거나 재시작 없이 UI 언어를 즉시 전환합니다"
        ]),
        ("Apple Music 原生", ["Apple Music 原生", "Apple Music Native", "Apple Music ネイティブ", "Apple Music 네이티브"]),
        ("Apple Music 网页", ["Apple Music 網頁", "Apple Music Web", "Apple Music Web", "Apple Music 웹"]),
        ("Apple Music 网页版", ["Apple Music 網頁版", "Apple Music Web", "Apple Music Web", "Apple Music 웹"]),
        ("网易云音乐", ["網易雲音樂", "NetEase Cloud", "NetEase Music", "넷이즈 클라우드"]),
        ("QQ 音乐", ["QQ 音樂", "QQ Music", "QQ Music", "QQ 뮤직"]),
        ("外置 SMTC", ["外置 SMTC", "External SMTC", "外部 SMTC", "외부 SMTC"]),
        ("外置媒体接管", ["外置媒體接管", "External Media", "外部メディア連携", "외부 미디어 연동"]),
        ("网易云 · 发现主页", ["網易雲 · 探索首頁", "NetEase · Home", "NetEase · ホーム", "넷이즈 · 홈"]),
        ("QQ 音乐 · 推荐主页", ["QQ 音樂 · 推薦首頁", "QQ Music · Home", "QQ Music · ホーム", "QQ 뮤직 · 홈"]),
        ("新碟与热歌广场", ["新碟與熱歌廣場", "New Releases & Hot", "新譜と人気チャート", "신곡 및 인기 차트"]),
        ("新碟首发与广场", ["新碟首發與廣場", "New Albums & Plaza", "新着アルバム", "최신 앨범"]),
        ("私人 FM 与电台", ["私人 FM 與電台", "Personal FM & Radio", "パーソナルFM・ラジオ", "개인 FM 및 라디오"]),
        ("QQ 音乐电台", ["QQ 音樂電台", "QQ Music Radio", "QQ Music ラジオ", "QQ 뮤직 라디오"]),
        ("私人 FM", ["私人 FM", "Personal FM", "パーソナルFM", "개인 FM"]),
        ("每日推荐", ["每日推薦", "Daily Mix", "毎日のおすすめ", "오늘의 추천"]),
        ("热歌榜", ["熱歌榜", "Hot Charts", "人気チャート", "인기 차트"]),
        ("QQ 热歌榜", ["QQ 熱歌榜", "QQ Hot Charts", "QQ 人気チャート", "QQ 인기 차트"]),
        ("QQ 电台", ["QQ 電台", "QQ Radio", "QQ ラジオ", "QQ 라디오"]),
        ("网易云 · 每日推荐歌曲", ["網易雲 · 每日推薦歌曲", "NetEase · Daily Picks", "NetEase · 毎日のおすすめ曲", "넷이즈 · 오늘의 추천곡"]),
        ("网易云 · 热门单曲", ["網易雲 · 熱門單曲", "NetEase · Top Tracks", "NetEase · 人気トラック", "넷이즈 · 인기 트랙"]),
        ("QQ 音乐 · 巅峰热歌", ["QQ 音樂 · 巔峰熱歌", "QQ Music · Top Hits", "QQ Music · トップヒット", "QQ 뮤직 · 인기 차트"]),
        ("资料库歌曲", ["資料庫歌曲", "Library Songs", "ライブラリの曲", "보관함 노래"]),

        // --- Backend Descriptions & Search Placeholders ---
        ("通过本机 Apple Music 后台服务播放 Hi-Res / 无损 ALAC，主页与资料库同步官方目录", [
            "透過本機 Apple Music 背景服務播放 Hi-Res / 無損 ALAC，首頁與資料庫同步官方目錄",
            "Plays Hi-Res / Lossless ALAC via the local Apple Music service with full official catalog & library sync",
            "ローカルApple MusicサービスでHi-Res/ロスレスALAC再生、公式カタログとライブラリ同期に対応",
            "로컬 Apple Music 서비스를 통해 Hi-Res / 무손실 ALAC 재생 및 공식 카탈로그·보관함 동기화"
        ]),
        ("通过内置 Edge MusicKit JS 网页引擎播放 Apple Music 完整曲库与电台，无需微软商店原版客户端", [
            "透過內建 Edge MusicKit JS 網頁引擎播放 Apple Music 完整曲庫與電台，無需微軟商店原版用戶端",
            "Plays full Apple Music catalog & radio via embedded Edge MusicKit JS — no Microsoft Store client required",
            "内蔵Edge MusicKit JSでApple Music全曲とラジオを再生（ストア版クライアント不要）",
            "내장 Edge MusicKit JS로 Apple Music 전체 음원 및 라디오 재생 (스토어 앱 불필요)"
        ]),
        ("原生流媒体解码直出，主页/发现/电台/搜索全面切换为网易云曲库，支持内置扫码登录与 YRC 逐字歌词", [
            "原生串流解碼直出，首頁/發現/電台/搜尋全面切換為網易雲曲庫，支援內建掃碼登入與 YRC 逐字歌詞",
            "Native MediaFoundation + WASAPI playback for NetEase Cloud Music with built-in QR login & YRC word lyrics",
            "NetEase MusicのネイティブWASAPI再生・QRログイン・YRC歌詞同期に対応",
            "넷이즈 클라우드 뮤직 네이티브 WASAPI 재생, 내장 QR 로그인 및 YRC 가사 지원"
        ]),
        ("原生流媒体解码直出，主页/发现/电台/搜索全面切换为 QQ 音乐巅峰榜与歌单，支持 Cookie 解锁高音质", [
            "原生串流解碼直出，首頁/發現/電台/搜尋全面切換為 QQ 音樂巔峰榜與歌單，支援 Cookie 解鎖高音質",
            "Native MediaFoundation + WASAPI playback for QQ Music charts & playlists with Cookie high-quality unlock",
            "QQ MusicのネイティブWASAPI再生、チャート・プレイリスト、Cookie高音質解除に対応",
            "QQ 뮤직 네이티브 WASAPI 재생, 인기 차트·플레이리스트 및 쿠키 고음질 지원"
        ]),
        ("只读/控制当前系统正在播放的任意音乐软件（Spotify / 客户端 / 浏览器），专注歌词与音频可视化", [
            "唯讀/控制目前系統正在播放的任意音樂軟體（Spotify / 用戶端 / 瀏覽器），專注歌詞與音訊視覺化",
            "Follows & controls any active Windows SMTC player (Spotify, browser, desktop apps) with lyrics & visualizer",
            "システム上で再生中の任意のプレイヤー（Spotify・ブラウザ等）と連携して歌詞・ビジュアライザを表示",
            "시스템에서 재생 중인 모든 음악 앱(Spotify, 브라우저 등)과 연동하여 가사 및 시각화 제공"
        ]),
        ("搜索 Apple Music", ["搜尋 Apple Music", "Search Apple Music", "Apple Music を検索", "Apple Music 검색"]),
        ("搜索网易云歌曲、专辑、歌单", ["搜尋網易雲歌曲、專輯、歌單", "Search NetEase songs, albums...", "NetEase の曲・アルバムを検索", "넷이즈 노래·앨범·플레이리스트 검색"]),
        ("搜索 QQ 音乐歌曲、专辑、歌单", ["搜尋 QQ 音樂歌曲、專輯、歌單", "Search QQ Music songs, albums...", "QQ Music の曲・アルバムを検索", "QQ 뮤직 노래·앨범·플레이리스트 검색"]),
        ("搜索曲库（切换对应后端播放）", ["搜尋曲庫（切換對應後端播放）", "Search catalog...", "カタログを検索...", "카탈로그 검색..."]),
        ("在左侧搜索栏输入歌曲、艺人或专辑名称（支持中文输入法与 Ctrl+V 粘贴）。", [
            "在左側搜尋欄輸入歌曲、藝人或專輯名稱（支援輸入法與 Ctrl+V 貼上）。",
            "Type a song, artist, or album in the search box on the left (supports IME & Ctrl+V).",
            "左の検索欄に曲名・アーティスト・アルバム名を入力してください。",
            "왼쪽 검색창에 노래, 아티스트 또는 앨범 이름을 입력하세요."
        ]),

        // --- Playback, Transport, Detail & Artist Pages ---
        ("播放", ["播放", "Play", "再生", "재생"]),
        ("加载中", ["載入中", "Loading", "読込中", "로딩 중"]),
        ("随机播放", ["隨機播放", "Shuffle", "シャッフル", "셔플"]),
        ("立即播放全部", ["立即播放全部", "Play All", "すべて再生", "전체 재생"]),
        ("下一首播放", ["下一首播放", "Play Next", "次に再生", "다음에 재생"]),
        ("添加到歌单 ›", ["加入播放清單 ›", "Add to Playlist ›", "プレイリストに追加 ›", "플레이리스트에 추가 ›"]),
        ("添加到歌单 ▾", ["加入播放清單 ▾", "Add to Playlist ▾", "プレイリストに追加 ▾", "플레이리스트에 추가 ▾"]),
        ("添加到资料库", ["加入資料庫", "Add to Library", "ライブラリに追加", "보관함에 추가"]),
        ("跨平台复制", ["跨平台複製", "Transfer", "別サービスへ複製", "플랫폼 복사"]),
        ("跨平台复制歌单...", ["跨平台複製歌單...", "Transfer Playlist...", "プレイリストを複製...", "플레이리스트 복사..."]),
        ("跨平台歌单复制", ["跨平台歌單複製", "Cross-Platform Playlist Transfer", "クロスプラットフォーム複製", "크로스 플랫폼 플레이리스트 복사"]),
        ("选择目标平台：", ["選擇目標平台：", "Destination platform:", "コピー先プラットフォーム：", "대상 플랫폼 선택:"]),
        ("开始跨平台复制", ["開始跨平台複製", "Start Transfer", "複製を開始", "복사 시작"]),
        ("切换平台并播放", ["切換平台並播放", "Switch & Play", "切替えて再生", "전환 후 재생"]),
        ("Apple Music 云端歌单", ["Apple Music 雲端歌單", "Apple Music Cloud", "Apple Music クラウド", "Apple Music 클라우드"]),
        ("网易云云端歌单", ["網易雲雲端歌單", "NetEase Cloud", "NetEase クラウド", "넷이즈 클라우드"]),
        ("QQ 音乐匹配队列", ["QQ 音樂配對佇列", "QQ Music Queue", "QQ Music キュー", "QQ 뮤직 재생목록"]),
        ("热门歌曲", ["熱門歌曲", "Top Songs", "トップソング", "인기 노래"]),
        ("专辑与单曲", ["專輯與單曲", "Albums & Singles", "アルバム＆シングル", "앨범 및 싱글"]),
        ("热门专辑", ["熱門專輯", "Top Albums", "人気アルバム", "인기 앨범"]),
        ("热门单曲", ["熱門單曲", "Top Songs", "人気ソング", "인기 곡"]),
        ("热门结果", ["熱門結果", "Top Results", "トップヒット", "상위 결과"]),
        ("精选歌单", ["精選歌單", "Featured Playlists", "おすすめプレイリスト", "추천 플레이리스트"]),
        ("精选推荐", ["精選推薦", "Featured", "ピックアップ", "추천"]),
        ("更新歌单", ["更新歌單", "Updated Playlist", "更新プレイリスト", "업데이트됨"]),
        ("新发行", ["新發行", "New Release", "ニューリリース", "신규 발매"]),
        ("Apple Music 艺人", ["Apple Music 藝人", "Apple Music Artist", "Apple Music アーティスト", "Apple Music 아티스트"]),
        ("资料库艺人 · 热门曲目与专辑", ["資料庫藝人 · 熱門曲目與專輯", "Library Artist · Top Songs & Albums", "ライブラリアーティスト", "보관함 아티스트"]),
        ("正在载入曲目列表...", ["正在載入曲目清單...", "Loading tracklist...", "トラックを読み込み中...", "트랙 목록 불러오는 중..."]),
        ("未能载入内容，请点击右上角刷新重试。", ["未能載入內容，請點擊右上角重新整理重試。", "Failed to load content. Click Refresh to retry.", "読み込みに失敗しました。更新ボタンを押して再試行してください。", "콘텐츠를 불러오지 못했습니다. 새로고침을 눌러 다시 시도하세요."]),
        ("暂无内容或正在加载中...", ["暫無內容或正在載入中...", "Loading or no items available...", "読み込み中、または項目がありません...", "항목이 없거나 불러오는 중..."]),
        ("暂无可写入歌单（请先登录）", ["暫無可寫入歌單（請先登入）", "No writable playlists (Sign in first)", "書込先プレイリストなし（要ログイン）", "추가할 플레이리스트 없음 (로그인 필요)"]),
        ("未在播放", ["未在播放", "Not Playing", "停止中", "재생 중이 아님"]),
        ("选择任意歌曲即可开始播放", ["選擇任意歌曲即可開始播放", "Pick any track to start listening", "曲を選んで再生を開始", "노래를 선택하여 재생하세요"]),
        ("选择曲目开始聆听", ["選擇曲目開始聆聽", "Choose a track to listen", "曲を選んで聴き始める", "트랙을 선택해 감상하세요"]),
        ("当前待播列表为空", ["目前待播清單為空", "Up Next queue is empty", "再生キューは空です", "재생 대기열이 비어 있습니다"]),
        ("打开音频与可视化设置...", ["開啟音訊與視覺化設定...", "Open Audio & Visualizer Settings...", "オーディオ・設定を開く...", "오디오 및 시각화 설정 열기..."]),
        ("连接个人资料库", ["連結個人資料庫", "Connect Personal Library", "ライブラリに接続", "개인 보관함 연결"]),
        ("前往设置", ["前往設定", "Open Settings", "設定を開く", "설정으로 이동"]),
        ("同步资料库歌曲...", ["同步資料庫歌曲...", "Syncing library songs...", "ライブラリ曲を同期中...", "보관함 노래 동기화 중..."]),
        ("正在同步资料库歌曲...", ["正在同步資料庫歌曲...", "Syncing library songs...", "ライブラリ曲を同期中...", "보관함 노래 동기화 중..."]),
        ("本地目录暂无已缓存的离线音频文件。", ["本機目錄暫無已快取的離線音訊檔案。", "No cached offline audio files found locally.", "キャッシュされたオフライン音源がありません。", "로컬에 캐시된 오프라인 오디오 파일이 없습니다."]),
        ("打开本地目录", ["開啟本機目錄", "Open Local Folder", "フォルダを開く", "로컬 폴더 열기"]),
        ("暂停下载队列", ["暫停下載佇列", "Pause Downloads", "ダウンロード一時停止", "다운로드 일시중지"]),
        ("恢复下载队列", ["恢復下載佇列", "Resume Downloads", "ダウンロード再開", "다운로드 재개"]),
        ("默认媒体目录", ["預設媒體目錄", "Default Media Folder", "既定メディアフォルダ", "기본 미디어 폴더"]),
        ("未知标题", ["未知標題", "Unknown Title", "タイトル不明", "제목 없음"]),

        // --- Lyrics Overlay & Controls ---
        ("AM Light · 正在匹配同步歌词...", ["AM Light · 正在配對同步歌詞...", "AM Light · Matching synced lyrics...", "AM Light · 同期歌詞を検索中...", "AM Light · 싱크 가사 매칭 중..."]),
        ("AM Light · 纯音乐", ["AM Light · 純音樂", "AM Light · Instrumental", "AM Light · インストゥルメンタル", "AM Light · 연주곡"]),
        ("正在匹配逐字同步歌词...", ["正在配對逐字同步歌詞...", "Matching word-by-word lyrics...", "タイム同期歌詞を検索中...", "글자별 싱크 가사 매칭 중..."]),
        ("暂无歌词", ["暫無歌詞", "No Lyrics Available", "歌詞がありません", "가사 없음"]),
        ("纯音乐", ["純音樂", "Instrumental", "インスト", "연주곡"]),
        ("译", ["譯", "Tr", "訳", "번역"]),
        ("流光", ["流光", "Aurora", "オーロラ", "오로라"]),
        ("柔焦", ["柔焦", "Soft", "ソフト", "소프트"]),
        ("沉浸", ["沉浸", "Focus", "没入", "몰입"]),
        ("导入", ["匯入", "Import", "読込", "불러오기"]),
        ("导入本地歌词 (.lrc / .ttml)...", ["匯入本機歌詞 (.lrc / .ttml)...", "Import Local Lyrics (.lrc / .ttml)...", "ローカル歌詞を読込 (.lrc/.ttml)...", "로컬 가사 불러오기 (.lrc/.ttml)..."]),
        ("开启歌词沉浸模式", ["開啟歌詞沉浸模式", "Enable Immersive Lyrics", "没入モードをオン", "가사 몰입 모드 켜기"]),
        ("关闭歌词沉浸模式", ["關閉歌詞沉浸模式", "Disable Immersive Lyrics", "没入モードをオフ", "가사 몰입 모드 끄기"]),
        ("自动词源", ["自動詞源", "Auto Src", "自動取得", "자동 소스"]),
        ("网易云逐字", ["網易雲逐字", "NetEase", "NetEase", "넷이즈"]),
        ("AMLL 词库", ["AMLL 詞庫", "AMLL DB", "AMLL", "AMLL"]),
        ("Apple Music 同步歌词", ["Apple Music 同步歌詞", "Apple Music Lyrics", "Apple Music 歌詞", "Apple Music 가사"]),
        ("Apple Music 逐词歌词", ["Apple Music 逐詞歌詞", "Apple Music Syllable Lyrics", "Apple Music タイム歌詞", "Apple Music 음절 가사"]),
        ("AMLL 同步歌词", ["AMLL 同步歌詞", "AMLL Synced Lyrics", "AMLL 同期歌詞", "AMLL 싱크 가사"]),
        ("AMLL 逐词歌词", ["AMLL 逐詞歌詞", "AMLL Word Lyrics", "AMLL ワード歌詞", "AMLL 단어 가사"]),
        ("LRCLIB 同步歌词", ["LRCLIB 同步歌詞", "LRCLIB Synced Lyrics", "LRCLIB 同期歌詞", "LRCLIB 싱크 가사"]),
        ("本地 TTML 逐词歌词", ["本機 TTML 逐詞歌詞", "Local TTML Lyrics", "ローカル TTML 歌詞", "로컬 TTML 가사"]),
        ("本地 YRC 逐字歌词", ["本機 YRC 逐字歌詞", "Local YRC Lyrics", "ローカル YRC 歌詞", "로컬 YRC 가사"]),
        ("本地 LRC 歌词", ["本機 LRC 歌詞", "Local LRC Lyrics", "ローカル LRC 歌詞", "로컬 LRC 가사"]),

        // --- Settings Sections & Controls ---
        ("Apple Music 账号与地区", ["Apple Music 帳號與地區", "Apple Music Account & Region", "Apple Music アカウントと地域", "Apple Music 계정 및 지역"]),
        ("网页版 Apple Music 播放引擎 (Edge MusicKit JS)", ["網頁版 Apple Music 播放引擎 (Edge MusicKit JS)", "Apple Music Web Engine (Edge MusicKit JS)", "Apple Music Web エンジン (Edge MusicKit JS)", "Apple Music 웹 엔진 (Edge MusicKit JS)"]),
        ("启动 / 网页登录", ["啟動 / 網頁登入", "Launch / Sign In", "起動 / ログイン", "실행 / 로그인"]),
        ("显示网页窗口", ["顯示網頁視窗", "Show Web Window", "ウィンドウ表示", "웹 창 표시"]),
        ("隐藏网页窗口", ["隱藏網頁視窗", "Hide Web Window", "ウィンドウ非表示", "웹 창 숨기기"]),
        ("同步网页 Token", ["同步網頁 Token", "Sync Web Token", "Token同期", "웹 토큰 동기화"]),
        ("关闭引擎", ["關閉引擎", "Stop Engine", "エンジン停止", "엔진 종료"]),
        ("使用本机 Edge 独立配置文件运行 MusicKit JS（支持 Widevine 完整曲库与电台，登录后自动同步个人资料库 Token）", [
            "使用本機 Edge 獨立設定檔執行 MusicKit JS（支援 Widevine 完整曲庫與電台，登入後自動同步個人資料庫 Token）",
            "Runs MusicKit JS in an isolated Edge profile with Widevine full catalog & radio; auto-syncs your library token after sign-in",
            "独立EdgeプロファイルでMusicKit JSを実行（Widevine全曲・ラジオ対応、ログイン後にTokenを自動同期）",
            "독립 Edge 프로필에서 MusicKit JS를 실행하며 로그인 후 보관함 토큰을 자동 동기화합니다"
        ]),
        ("网易云音乐账号与音质", ["網易雲音樂帳號與音質", "NetEase Account & Quality", "NetEase アカウントと音質", "넷이즈 계정 및 음질"]),
        ("网易云账号", ["網易雲帳號", "NetEase Account", "NetEase アカウント", "넷이즈 계정"]),
        ("网易云音质", ["網易雲音質", "NetEase Quality", "NetEase 音質", "넷이즈 음질"]),
        ("扫码登录", ["掃碼登入", "QR Login", "QRログイン", "QR 로그인"]),
        ("粘贴 Cookie", ["貼上 Cookie", "Paste Cookie", "Cookie貼付", "쿠키 붙여넣기"]),
        ("网页端", ["網頁端", "Web", "Web", "웹"]),
        ("退出", ["登出", "Sign Out", "ログアウト", "로그아웃"]),
        ("刷新二维码", ["重新整理二維碼", "Refresh QR", "QR更新", "QR 새로고침"]),
        ("打开手机「网易云音乐」扫一扫", ["開啟手機「網易雲音樂」掃一掃", "Scan with NetEase Cloud Music App", "スマホ版 NetEase Music でスキャン", "모바일 넷이즈 뮤직 앱으로 스캔"]),
        ("扫码确认后自动获取 MUSIC_U 并同步你的自建/收藏歌单、每日推荐与 VIP 音质。", [
            "掃碼確認後自動取得 MUSIC_U 並同步你的自建/收藏歌單、每日推薦與 VIP 音質。",
            "Automatically retrieves MUSIC_U and syncs your playlists, Daily Picks, and VIP audio quality after scanning.",
            "スキャン確認後にMUSIC_Uを自動取得し、プレイリスト・おすすめ・VIP音質を同期します。",
            "스캔 확인 후 MUSIC_U를 자동 저장하고 플레이리스트, 오늘의 추천 및 VIP 음질을 동기화합니다."
        ]),
        ("音频通过 MediaFoundation + WASAPI 原生直出，自动优先加载网易云精确曲目 ID 逐字歌词 (YRC) 与翻译。", [
            "音訊透過 MediaFoundation + WASAPI 原生直出，自動優先載入網易雲精確曲目 ID 逐字歌詞 (YRC) 與翻譯。",
            "Direct MediaFoundation + WASAPI output; automatically prioritizes exact-ID NetEase YRC word-by-word lyrics & translations.",
            "MediaFoundation + WASAPIで直接出力し、曲ID一致のYRC逐字歌詞と翻訳を優先読み込みします。",
            "MediaFoundation + WASAPI 직출력으로 정확한 곡 ID 기반 YRC 가사와 번역을 우선 불러옵니다."
        ]),
        ("QQ 音乐账号与音质", ["QQ 音樂帳號與音質", "QQ Music Account & Quality", "QQ Music アカウントと音質", "QQ 뮤직 계정 및 음질"]),
        ("QQ 音乐账号", ["QQ 音樂帳號", "QQ Music Account", "QQ Music アカウント", "QQ 뮤직 계정"]),
        ("QQ 音乐音质", ["QQ 音樂音質", "QQ Music Quality", "QQ Music 音質", "QQ 뮤직 음질"]),
        ("清除", ["清除", "Clear", "クリア", "지우기"]),
        ("打开 y.qq.com", ["開啟 y.qq.com", "Open y.qq.com", "y.qq.com を開く", "y.qq.com 열기"]),
        ("粘贴包含 uin 与 qm_keyst 的浏览器 Cookie 可同步个人「我喜欢」与歌单；若选高音质无权限会自动平滑降级播放。", [
            "貼上包含 uin 與 qm_keyst 的瀏覽器 Cookie 可同步個人「我喜歡」與歌單；若選高音質無權限會自動平滑降級播放。",
            "Paste browser cookies containing uin & qm_keyst to sync playlists; gracefully falls back to available quality if VIP is required.",
            "uinとqm_keystを含むCookieを貼り付けるとプレイリスト同期が可能（権限不足時は自動で再生可能音質に切替）。",
            "uin과 qm_keyst가 포함된 쿠키를 붙여넣으면 플레이리스트를 동기화하며 권한 부족 시 재생 가능한 음질로 자동 전환됩니다."
        ]),
        ("通用外置播放器接管 (Windows SMTC)", ["通用外置播放器接管 (Windows SMTC)", "External Media Session Bridge (Windows SMTC)", "外部プレイヤー連携 (Windows SMTC)", "외부 플레이어 연동 (Windows SMTC)"]),
        ("此模式不会抢占音频焦点，完整保留 AM Light 的逐字滚动歌词与实时音频可视化分析。", [
            "此模式不會搶佔音訊焦點，完整保留 AM Light 的逐字滾動歌詞與即時音訊視覺化分析。",
            "Non-intrusive mode: never steals audio focus while preserving AM Light's synced lyrics & real-time audio visualizer.",
            "オーディオフォーカスを奪わず、AM Lightの同期歌詞とリアルタイム可視化をそのまま利用できます。",
            "오디오 포커스를 뺏지 않고 AM Light의 싱크 가사와 실시간 오디오 시각화를 그대로 유지합니다."
        ]),
        ("原装客户端账号", ["原裝用戶端帳號", "Official Account", "公式アカウント", "공식 계정"]),
        ("未检测到桌面版 Apple Music 登录记录", [
            "未偵測到桌面版 Apple Music 登入紀錄",
            "Official Apple Music login not detected",
            "公式 Apple Music のログイン未検出",
            "데스크톱 Apple Music 로그인 기록 없음"
        ]),
        ("同步原装端", ["同步原裝端", "Sync Client", "公式端同期", "클라이언트 동기화"]),
        ("内容地区", ["內容地區", "Storefront", "ストア地域", "스토어 지역"]),
        ("个人资料库授权 (Media-User-Token)：未配置", ["個人資料庫授權 (Media-User-Token)：未設定", "Library Token (Media-User-Token): Not configured", "ライブラリToken (Media-User-Token)：未設定", "보관함 토큰 (Media-User-Token): 미설정"]),
        ("打开网页端", ["開啟網頁端", "Open Web", "Webを開く", "웹 열기"]),
        ("粘贴保存", ["貼上儲存", "Paste & Save", "貼付して保存", "붙여넣기 저장"]),
        ("验证", ["驗證", "Verify", "検証", "확인"]),
        ("点击此处按 Ctrl+V 粘贴 Media-User-Token（用于同步云端歌单与收藏专辑）", [
            "點擊此處按 Ctrl+V 貼上 Media-User-Token（用於同步雲端歌單與收藏專輯）",
            "Click here and press Ctrl+V to paste Media-User-Token (for cloud playlists & library)",
            "ここをクリックして Ctrl+V で Media-User-Token を貼り付け（クラウド同期用）",
            "여기를 클릭하고 Ctrl+V로 Media-User-Token 붙여넣기 (클라우드 동기화)"
        ]),
        ("播放故障诊断与 DRM 修复", ["播放故障診斷與 DRM 修復", "Playback Diagnostics & DRM Repair", "再生トラブル診断・DRM修復", "재생 진단 및 DRM 복구"]),
        ("原版 Apple Music 无法播放 / 点歌秒切修复", ["原版 Apple Music 無法播放 / 點歌秒切修復", "Fix Apple Music 0:00 Skip / DRM Permission Issue", "Apple Music 再生不可・0秒スキップ修復", "Apple Music 0초 스킵 / DRM 권한 복구"]),
        ("支持无管理员权限快速检测，或一键备份并修复目录写入权限与第三方重定向", [
            "支援無管理員權限快速檢測，或一鍵備份並修復目錄寫入權限與第三方重新導向",
            "Read-only check without elevation, or 1-click backup & repair of Apple Computer ACLs and third-party DRM redirects",
            "管理者権限なしで即時診断、またはワンクリックで権限修復とバックアップを実行",
            "관리자 권한 없이 즉시 진단하거나 원클릭으로 백업 및 DRM 권한 복구 수행"
        ]),
        ("重新检测", ["重新檢測", "Re-check", "再診断", "다시 검사"]),
        ("一键修复", ["一鍵修復", "1-Click Repair", "ワンクリック修復", "원클릭 복구"]),
        ("修复中...", ["修復中...", "Repairing...", "修復中...", "복구 중..."]),
        ("项目主页", ["專案首頁", "Repair Repo", "詳細ページ", "복구 저장소"]),

        // --- Settings: Audio, Lyrics, Visualizer, Appearance, About ---
        ("音频与播放", ["音訊與播放", "Audio & Playback", "オーディオと再生", "오디오 및 재생"]),
        ("播放方式", ["播放方式", "Playback Engine", "再生エンジン", "재생 방식"]),
        ("内置播放服务", ["內建播放服務", "Built-in Service", "内蔵サービス", "내장 서비스"]),
        ("Apple Music 应用", ["Apple Music 應用", "Apple Music App", "Apple Music アプリ", "Apple Music 앱"]),
        ("释放会话", ["釋放工作階段", "Disconnect", "セッション解放", "세션 해제"]),
        ("内置服务已连接", ["內建服務已連線", "Service Connected", "サービス接続中", "서비스 연결됨"]),
        ("按需自动连接", ["按需自動連線", "Auto-connect on play", "再生時に自動接続", "재생 시 자동 연결"]),
        ("音频质量", ["音訊品質", "Audio Quality", "オーディオ品質", "오디오 품질"]),
        ("高效 AAC", ["高效 AAC", "HE-AAC", "高効率 AAC", "고효율 AAC"]),
        ("高质量 AAC", ["高品質 AAC", "High Quality AAC", "高音質 AAC", "고음질 AAC"]),
        ("无损 (24/48)", ["無損 (24/48)", "Lossless (24/48)", "ロスレス (24/48)", "무손실 (24/48)"]),
        ("Hi-Res 无损 (24/192)", ["Hi-Res 無損 (24/192)", "Hi-Res (24/192)", "ハイレゾ (24/192)", "Hi-Res (24/192)"]),
        ("标准 128k", ["標準 128k", "Standard 128k", "標準 128k", "표준 128k"]),
        ("极高 320k", ["極高 320k", "HQ 320k", "高音質 320k", "고음질 320k"]),
        ("无损 FLAC", ["無損 FLAC", "Lossless FLAC", "ロスレス FLAC", "무손실 FLAC"]),
        ("Hi-Res 无损", ["Hi-Res 無損", "Hi-Res Lossless", "ハイレゾロスレス", "Hi-Res 무손실"]),
        ("SQ 无损 FLAC", ["SQ 無損 FLAC", "SQ Lossless FLAC", "SQ ロスレス FLAC", "SQ 무손실 FLAC"]),
        ("标准 128k MP3", ["標準 128k MP3", "Standard 128k MP3", "標準 128k MP3", "표준 128k MP3"]),
        ("M4A 标准", ["M4A 標準", "M4A Standard", "M4A 標準", "M4A 표준"]),
        ("音频输出设备", ["音訊輸出裝置", "Audio Output Device", "オーディオ出力デバイス", "오디오 출력 장치"]),
        ("系统默认输出设备", ["系統預設輸出裝置", "System Default Output", "システム既定デバイス", "시스템 기본 출력 장치"]),
        ("默认输出设备", ["預設輸出裝置", "Default Output", "既定デバイス", "기본 출력 장치"]),
        ("切换设备", ["切換裝置", "Switch", "切替", "전환"]),
        ("音量平衡", ["音量平衡", "Sound Check", "サウンドチェック", "음량 자동 조절"]),
        ("歌曲交叉渐入渐出", ["歌曲交叉漸入漸出", "Crossfade Songs", "クロスフェード", "크로스페이드"]),

        ("歌词显示与动效", ["歌詞顯示與動效", "Lyrics Display & Motion", "歌詞表示とアニメーション", "가사 표시 및 애니메이션"]),
        ("歌词页面刷新率", ["歌詞頁面更新率", "Lyrics Refresh Rate", "歌詞画面リフレッシュレート", "가사 화면 주사율"]),
        ("可视化与动态封面", ["視覺化與動態封面", "Visualizer & Motion Art", "ビジュアライザと動的アート", "시각화 및 모션 커버"]),
        ("可视化页面刷新率", ["視覺化頁面更新率", "Visualizer Refresh Rate", "ビジュアライザ更新レート", "시각화 화면 주사율"]),
        ("自适应 [60/72]", ["自適應 [60/72]", "Adaptive [60/72]", "自動 [60/72]", "자동 [60/72]"]),
        ("垂直同步", ["垂直同步", "VSync", "VSync", "수직동기화"]),
        ("自定义", ["自訂", "Custom", "カスタム", "사용자 지정"]),
        ("后台失焦时歌词页自动降频（60/72 自适应）", [
            "背景失焦時歌詞頁自動降頻（60/72 自適應）",
            "Throttle lyrics FPS when unfocused (60/72 Adaptive)",
            "非アクティブ時に歌詞画面のFPSを自動制限 (60/72)",
            "백그라운드 시 가사 화면 주사율 자동 절전 (60/72)"
        ]),
        ("默认关闭（窗口失去焦点仍满帧渲染）；开启后失焦自动切换为 60/72 自适应省电", [
            "預設關閉（視窗失去焦點仍滿幀算繪）；開啟後失焦自動切換為 60/72 自適應省電",
            "Off by default (full FPS even unfocused); enable to drop to 60/72 Adaptive for power saving",
            "既定オフ（非アクティブでもフルFPS）。オンで省電力モードへ自動切替",
            "기본 꺼짐(비활성 시에도 최대 FPS 유지). 켜면 60/72 절전 모드로 자동 전환"
        ]),
        ("西文歌词按词律动", ["西文歌詞按詞律動", "Latin Word-by-Word Motion", "欧文歌詞を単語単位でアニメーション", "영문 가사 단어 단위 모션"]),
        ("英文以完整单词为单位扫光与上浮，关闭后按单字母细分", [
            "英文以完整單字為單位掃光與上浮，關閉後按單字母細分",
            "Sweep glow & lift per whole word for Latin lyrics; turn off for per-letter animation",
            "英単語単位で光と浮き上がりを描画（オフで1文字単位）",
            "영문 가사를 단어 단위로 강조 및 모션 처리 (끄면 글자 단위)"
        ]),
        ("自动检测歌词强调", ["自動偵測歌詞強調", "Auto Emphasis Detection", "強調フレーズ自動検出", "강조 가사 자동 감지"]),
        ("开启时对长拖音自动触发光效；关闭仅保留歌词原标记强调", [
            "開啟時對長拖音自動觸發光效；關閉僅保留歌詞原標記強調",
            "Automatically adds glow on long sustained notes; off keeps explicit lyric emphasis tags only",
            "長い伸ばし音に自動で光彩を付与（オフで元タグのみ）",
            "긴 여음에 자동으로 빛 효과 적용 (끄면 원본 강조 태그만 유지)"
        ]),
        ("歌词进度边界弹性波浪", ["歌詞進度邊界彈性波浪", "Frontier Elastic Wave", "進捗フロンティア弾性ウェーブ", "가사 경계 탄성 웨이브"]),
        ("沿高亮前沿微倾上浮防中文字割裂，关闭则整字整体上浮", [
            "沿高亮前沿微傾上浮防中文字割裂，關閉則整字整體上浮",
            "Smooth frontier tilt-lift that prevents CJK glyph tearing; off lifts whole characters uniformly",
            "ハイライト境界に沿って滑らかに傾き上昇（文字割れを防止）",
            "강조 경계를 따라 부드럽게 상승하여 글자 깨짐을 방지합니다"
        ]),
        ("歌词页流动封面纹理 (FreeBlur)", ["歌詞頁流動封面紋理 (FreeBlur)", "Flowing Cover Mesh (FreeBlur)", "流動アートワーク背景 (FreeBlur)", "유동 커버 배경 (FreeBlur)"]),
        ("开启后减轻背景模糊以保留流动封面轮廓；关闭为默认柔和扩散流光", [
            "開啟後減輕背景模糊以保留流動封面輪廓；關閉為預設柔和擴散流光",
            "Lighter blur that preserves flowing cover contours; off uses soft diffused aurora glow",
            "ぼかしを弱めてジャケットの流動感を強調（オフで柔らかなオーロラ）",
            "블러를 줄여 앨범 커버의 유동적인 윤곽을 유지합니다"
        ]),
        ("末尾停留整词渐亮", ["末尾停留整詞漸亮", "End-Hold Full-Word Glow", "語尾ホールド全体発光", "끝 머무름 전체 발광"]),
        ("高亮进度停在强调字词末尾时，整词发光逐渐增强至前沿亮度", [
            "高亮進度停在強調字詞末尾時，整詞發光逐漸增強至前沿亮度",
            "Gradually brightens the entire emphasized word while holding at its trailing boundary",
            "強調ワードの末尾で一時停止中、単語全体の発光を徐々に強めます",
            "강조 단어 끝에 머무를 때 단어 전체의 빛이 점진적으로 밝아집니다"
        ]),
        ("未唱歌词轻度降采样节能", ["未唱歌詞輕度降採樣節能", "Downsample Inactive Lyrics", "未再生歌詞の軽量ダウンサンプリング", "비활성 가사 저전력 다운샘플링"]),
        ("关闭为原生 1× 高清景深模糊；开启后未唱行 0.75× 渲染节能", [
            "關閉為原生 1× 高清景深模糊；開啟後未唱行 0.75× 算繪節能",
            "Off renders crisp 1× depth-of-field blur; on renders inactive lines at 0.75× to save GPU power",
            "オフで1×高精細ぼかし、オンで非アクティブ行を0.75×描画して省電力化",
            "끄면 1× 선명한 심도 블러, 켜면 비활성 행을 0.75×로 렌더링해 GPU 절약"
        ]),
        ("歌词页沉浸模式", ["歌詞頁沉浸模式", "Lyrics Immersive Mode", "歌詞没入モード", "가사 몰입 모드"]),
        ("开启后隐藏歌词页四角所有控件，仅保留背景、封面、播放控制与歌词；鼠标移近角落自动渐显", [
            "開啟後隱藏歌詞頁四角所有控制項，僅保留背景、封面、播放控制與歌詞；滑鼠移近角落自動漸顯",
            "Hides all corner chrome on the lyrics screen, keeping only cover, playback, lyrics & aurora; hover near a corner to reveal",
            "四隅のUIを非表示にして背景・ジャケット・再生・歌詞のみを表示（カーソル接近で自動表示）",
            "네 모서리 UI를 숨기고 배경·커버·재생 컨트롤·가사만 표시 (마우스 접근 시 자동 표시)"
        ]),
        ("非强调歌词附加发光 / 放大", ["非強調歌詞附加發光 / 放大", "Non-Emphasis Glow / Scale", "通常歌詞の発光・拡大", "일반 가사 발광 / 확대"]),
        ("默认仅上浮，仅强调词句高亮放大与发光", [
            "預設僅上浮，僅強調詞句高亮放大與發光",
            "Subtle lift only by default; glow & scale reserved for emphasized phrases",
            "既定では浮き上がりのみ。強調フレーズに発光・拡大を適用",
            "기본은 상승 모션만 적용되며 강조 구절에만 발광/확대가 적용됩니다"
        ]),
        ("关闭（默认）", ["關閉（預設）", "Off (Default)", "オフ（既定）", "꺼짐 (기본)"]),
        ("仅发光", ["僅發光", "Glow Only", "発光のみ", "발광만"]),
        ("仅放大", ["僅放大", "Scale Only", "拡大のみ", "확대만"]),
        ("发光 + 放大", ["發光 + 放大", "Glow + Scale", "発光＋拡大", "발광 + 확대"]),
        ("动态封面（实验性）", ["動態封面（實驗性）", "Motion Artwork", "モーションアートワーク", "모션 아트워크"]),
        ("仅在主页与歌词页启用纯 Win32 硬件解码动态封面，新发现与广播保持静态高清封面", [
            "僅在首頁與歌詞頁啟用純 Win32 硬體解碼動態封面，新發現與廣播保持靜態高清封面",
            "Hardware-decoded animated album covers on Home, Album/Playlist Detail & Lyrics; static HD covers elsewhere",
            "ホーム・詳細・歌詞画面でハードウェアデコード動的カバーを再生",
            "홈, 상세 페이지 및 가사 화면에서 하드웨어 디코딩 모션 커버 재생"
        ]),
        ("实时频谱播放图标", ["即時頻譜播放圖示", "Live EQ Icon Spectrum", "リアルタイムEQアイコン", "실시간 스펙트럼 아이콘"]),
        ("未打开可视化页时，播放封面五条竖线与侧栏可视化图标仍随实际音频实时跳动；关闭可停用后台采样", [
            "未開啟視覺化頁時，播放封面五條豎線與側欄視覺化圖示仍隨實際音訊即時跳動；關閉可停用背景採樣",
            "Animates playing cover bars & sidebar Visualizer icon to live audio even when Visualizer is closed",
            "ビジュアライザ非表示時も再生バーとアイコンを実音声に連動（オフでバックグラウンド計測停止）",
            "시각화 페이지가 닫혀 있어도 커버 이퀄라이저와 아이콘이 실시간 오디오에 반응합니다"
        ]),

        ("外观、性能与内存", ["外觀、效能與記憶體", "Appearance, Performance & Memory", "外観・パフォーマンス・メモリ", "외관, 성능 및 메모리"]),
        ("沉浸式窗口边框圆角", ["沉浸式視窗邊框圓角", "Window Corner Radius", "ウィンドウ角丸半径", "창 모서리 둥글기"]),
        ("直角 0px", ["直角 0px", "Sharp 0px", "直角 0px", "직각 0px"]),
        ("默认 12px", ["預設 12px", "Default 12px", "既定 12px", "기본 12px"]),
        ("大圆角 22px", ["大圓角 22px", "Rounded 22px", "大角丸 22px", "둥글게 22px"]),
        ("液态玻璃材质", ["液態玻璃材質", "Liquid Glass Material", "リキッドガラス質感", "리퀴드 글래스 재질"]),
        ("默认关闭；开启后侧栏与控件启用实时折射与流光边", [
            "預設關閉；開啟後側欄與控制項啟用即時折射與流光邊",
            "Off by default; enables live backdrop refraction and specular rims on sidebar & floating controls",
            "既定オフ。オンでサイドバーとボタンにリアルタイム屈折ガラスを適用",
            "기본 꺼짐. 켜면 사이드바와 컨트롤에 실시간 굴절 유리 효과를 적용합니다"
        ]),
        ("减弱动态效果", ["減弱動態效果", "Reduce Motion", "視差・アニメ軽減", "동작 줄이기"]),
        ("收敛弹簧、封面展开与粒子运动（不影响歌词页流光背景）", [
            "收斂彈簧、封面展開與粒子運動（不影響歌詞頁流光背景）",
            "Dampens springs, cover transitions & particle motion (preserves lyrics aurora background)",
            "スプリング・展開アニメ・粒子を抑制（歌詞背景のオーロラは維持）",
            "스프링, 커버 전환 및 파티클 동작을 줄입니다 (가사 오로라 배경 유지)"
        ]),
        ("降低透明度", ["降低透明度", "Reduce Transparency", "透明度を下げる", "투명도 줄이기"]),
        ("将侧栏与浮层切换为纯色表面（不影响歌词页流光背景）", [
            "將側欄與浮層切換為純色表面（不影響歌詞頁流光背景）",
            "Switches sidebar & popovers to solid surfaces (preserves lyrics aurora background)",
            "サイドバーやポップオーバーを不透明表示に変更（歌詞オーロラ背景は維持）",
            "사이드바와 팝오버를 불투명 표면으로 전환합니다 (가사 오로라 배경 유지)"
        ]),
        ("增强内存释放", ["增強記憶體釋放", "Aggressive Memory Trim", "メモリ解放強化", "메모리 해제 강화"]),
        ("高频回收封面/缓存并压缩工作集（推荐与减弱动态效果一并开启）", [
            "高頻回收封面/快取並壓縮工作集（推薦與減弱動態效果一併開啟）",
            "Frequently trims artwork/GPU caches & working set to minimize RAM usage",
            "アートワーク・GPUキャッシュとワーキングセットを積極的に解放して省メモリ化",
            "커버·GPU 캐시와 작업 집합을 자주 정리하여 메모리 사용량을 최소화합니다"
        ]),

        // --- Spatial Audio Disclosure ---
        ("空间音频与声场选项", ["空間音訊與聲場選項", "Spatial Audio & Soundstage", "空間オーディオと音場設定", "공간 음향 및 사운드스테이지"]),
        ("杜比全景声", ["杜比全景聲", "Dolby Atmos", "Dolby Atmos", "Dolby Atmos"]),
        ("自动", ["自動", "Auto", "自動", "자동"]),
        ("始终开启", ["始終開啟", "Always On", "常にオン", "항상 켜기"]),
        ("关闭", ["關閉", "Off", "オフ", "끄기"]),
        ("本地双耳空间渲染 (HRTF)", ["本機雙耳空間算繪 (HRTF)", "Local Binaural HRTF", "ローカルバイノーラル (HRTF)", "로컬 바이노럴 (HRTF)"]),
        ("对立体声信号进行外化头影与早期反射渲染（需两路可用输出端点）", [
            "對立體聲訊號進行外化頭影與早期反射算繪（需兩路可用輸出端點）",
            "Externalizes stereo audio with HRTF head-shadow & early reflections (requires 2 endpoints)",
            "ステレオ信号にHRTF頭部伝達関数と初期反射を適用",
            "스테레오 신호에 HRTF 및 초기 반사 공간 렌더링 적용"
        ]),
        ("直通中", ["直通中", "Bypassed", "バイパス中", "바이패스 중"]),
        ("直通对比", ["直通對比", "Bypass A/B", "バイパス比較", "바이패스 비교"]),
        ("声场风味预设", ["聲場風味預設", "Spatial Presets", "音場プリセット", "음장 프리셋"]),
        ("影院空间", ["影院空間", "Cinema", "シネマ", "시네마"]),
        ("近场监听", ["近場監聽", "Nearfield", "ニアフィールド", "니어필드"]),
        ("音乐厅", ["音樂廳", "Concert Hall", "コンサートホール", "콘서트홀"]),
        ("人声聚焦", ["人聲聚焦", "Intimate Vocal", "ボーカル重視", "보컬 포커스"]),

        // --- About & Updates ---
        ("关于与更新", ["關於與更新", "About & Updates", "情報とアップデート", "정보 및 업데이트"]),
        ("检查更新", ["檢查更新", "Check Updates", "更新確認", "업데이트 확인"]),
        ("正在检查...", ["正在檢查...", "Checking...", "確認中...", "확인 중..."]),
        ("GitHub 仓库", ["GitHub 儲存庫", "GitHub Repo", "GitHub リポジトリ", "GitHub 저장소"]),
        ("前往下载新版本", ["前往下載新版本", "Download Update", "新版をダウンロード", "업데이트 다운로드"]),
        ("点击「检查更新」向 GitHub Releases 查询最新版本", [
            "點擊「檢查更新」向 GitHub Releases 查詢最新版本",
            "Click 'Check Updates' to query the latest release on GitHub Releases",
            "「更新確認」をクリックして GitHub Releases の最新版を確認",
            "'업데이트 확인'을 눌러 GitHub Releases의 최신 버전을 확인하세요"
        ]),
        ("歌词动效与同步参考了 LyricsBlossom 及其作者分享的资料", [
            "歌詞動效與同步參考了 LyricsBlossom 及其作者分享的資料",
            "Lyrics motion & sync inspired by LyricsBlossom and its author's technical insights",
            "歌詞アニメーションは LyricsBlossom の設計思想を参考にしています",
            "가사 모션 및 동기화는 LyricsBlossom의 설계를 참고했습니다"
        ]),

        // --- Visualizer Modes, Audio Inspector & Badges ---
        ("音质鉴真", ["音質鑑真", "Audio Inspector", "音質鑑定", "음질 감별"]),
        ("频谱与示波", ["頻譜與示波", "Spectrum & Scope", "スペクトラム", "스펙트럼"]),
        ("声场图", ["聲場圖", "Soundstage", "音場マップ", "사운드스테이지"]),
        ("反应堆芯", ["反應堆芯", "Reactor Core", "リアクター", "리액터 코어"]),
        ("流光丝绸", ["流光絲綢", "Silk Aurora", "シルクオーロラ", "실크 오로라"]),
        ("律动圆环", ["律動圓環", "Rhythm Ring", "リズムリング", "리듬 링"]),
        ("迷幻极光 (WMP Alchemy)", ["迷幻極光 (WMP Alchemy)", "Alchemy Aurora", "アルケミー極光", "알케미 오로라"]),
        ("条形和波浪: 翠绿电光 (Emerald)", ["條形和波浪: 翠綠電光 (Emerald)", "Bars & Waves: Emerald", "バー＆ウェーブ: エメラルド", "바 & 웨이브: 에메랄드"]),
        ("条形和波浪: 烈焰 (Fire)", ["條形和波浪: 烈焰 (Fire)", "Bars & Waves: Fire", "バー＆ウェーブ: ファイア", "바 & 웨이브: 파이어"]),
        ("XP 复古", ["XP 復古", "XP Retro", "XP レトロ", "XP 레트로"]),
        ("系统声音 (System)", ["系統聲音 (System)", "System Audio", "システム音声", "시스템 오디오"]),
        ("采样中", ["採樣中", "Sampling", "計測中", "샘플링 중"]),
        ("分析中", ["分析中", "Analyzing", "分析中", "분석 중"]),
        ("分析中...", ["分析中...", "Analyzing...", "分析中...", "분석 중..."]),
        ("等待音频", ["等待音訊", "Awaiting Audio", "音声待機中", "오디오 대기 중"]),
        ("等待音频瞬态包络", ["等待音訊瞬態包絡", "Awaiting audio transients", "音声トランジェント待機中", "오디오 신호 대기 중"]),
        ("静音待命", ["靜音待命", "Silent Standby", "無音待機", "무음 대기"]),
        ("正在监听系统音频流", ["正在監聽系統音訊串流", "Monitoring system audio stream", "システム音声ストリームを監視中", "시스템 오디오 스트림 모니터링 중"]),
        ("冻结", ["凍結", "Freeze", "一時停止", "정지"]),
        ("恢复", ["恢復", "Resume", "再開", "재개"]),
        ("重测", ["重測", "Reset", "リセット", "재측정"]),
        ("自动清零", ["自動清零", "Auto Reset", "自動リセット", "자동 초기화"]),
        ("保持", ["保持", "Hold", "保持", "유지"]),
        ("单色", ["單色", "Mono", "単色", "단색"]),
        ("低", ["低", "Sub", "低域", "저음"]),
        ("中低", ["中低", "Low", "中低", "중저"]),
        ("中", ["中", "Mid", "中域", "중음"]),
        ("中高", ["中高", "High", "中高", "중고"]),
        ("高", ["高", "Air", "高域", "고음"]),
        ("慢速", ["慢速", "Slow", "低速", "느림"]),
        ("加快", ["加快", "Fast", "高速", "빠름"]),
        ("控制棒视图 · 点击画面切换立体发光堆芯", [
            "控制棒視圖 · 點擊畫面切換立體發光堆芯",
            "Control Rods · Click canvas for 3D Core",
            "制御棒ビュー・クリックで立体コアへ切替",
            "제어봉 뷰 · 클릭하여 3D 코어로 전환"
        ]),
        ("立体发光堆芯 · 点击画面切换控制棒视图", [
            "立體發光堆芯 · 點擊畫面切換控制棒視圖",
            "3D Reactor Core · Click canvas for Control Rods",
            "立体コア・クリックで制御棒ビューへ切替",
            "3D 코어 · 클릭하여 제어봉 뷰로 전환"
        ]),
        ("高解析度无损", ["高解析度無損", "Hi-Res Lossless", "ハイレゾロスレス", "고해상도 무손실"]),
        ("无损", ["無損", "Lossless", "ロスレス", "무손실"]),
        ("无损 Lossless", ["無損 Lossless", "Lossless", "ロスレス", "무손실"]),
        ("CD 无损", ["CD 無損", "CD Lossless", "CD ロスレス", "CD 무손실"]),
        ("无损 PCM", ["無損 PCM", "Lossless PCM", "ロスレス PCM", "무손실 PCM"]),
        ("无损音频", ["無損音訊", "Lossless Audio", "ロスレスオーディオ", "무손실 오디오"]),
        ("高码率有损", ["高碼率有損", "High-Bitrate Lossy", "高ビットレート非可逆", "고비트레이트 손실"]),
        ("标准有损", ["標準有損", "Standard Lossy", "標準非可逆", "표준 손실"]),
        ("中等有损", ["中等有損", "Medium Lossy", "中ビットレート非可逆", "중간 손실"]),
        ("低码率有损", ["低碼率有損", "Low-Bitrate Lossy", "低ビットレート非可逆", "저비트레이트 손실"]),
        ("声学段落", ["聲學段落", "Acoustic Passage", "アコースティック区間", "어쿠스틱 구간"]),
        ("2.0 立体声", ["2.0 立體聲", "2.0 Stereo", "2.0 ステレオ", "2.0 스테레오"]),
        ("16ch JOC 空间音频", ["16ch JOC 空間音訊", "16ch JOC Spatial", "16ch JOC 空間オーディオ", "16ch JOC 공간 음향"]),
        ("高效省流 AAC", ["高效省流 AAC", "High-Efficiency AAC", "高効率 AAC", "고효율 AAC"]),
        ("开始播放后将实时采样输出信号，分析有效频谱上限、MDCT 子带连续度与量化位深特征。", [
            "開始播放後將即時採樣輸出訊號，分析有效頻譜上限、MDCT 子帶連續度與量化位深特徵。",
            "Start playback to sample real-time audio output, measuring bandwidth ceiling, MDCT continuity & bit-depth.",
            "再生を開始するとリアルタイムに信号を計測し、帯域上限・MDCT連続性・ビット深度を解析します。",
            "재생을 시작하면 실시간 오디오 출력을 분석하여 대역폭 상한, MDCT 연속성 및 비트 심도를 측정합니다."
        ]),

        // --- Common Status & Banner Notices ---
        ("正在缓冲音频流…", ["正在緩衝音訊串流…", "Buffering audio stream...", "オーディオをバッファリング中...", "오디오 스트림 버퍼링 중..."]),
        ("正在连接音源准备播放…", ["正在連線音源準備播放…", "Connecting audio source...", "音源に接続中...", "음원 연결 중..."]),
        ("已添加到资料库", ["已加入資料庫", "Added to Library", "ライブラリに追加しました", "보관함에 추가되었습니다"]),
        ("已开启歌词沉浸模式（鼠标移近四角时自动显示控件）", [
            "已開啟歌詞沉浸模式（滑鼠移近四角時自動顯示控制項）",
            "Immersive lyrics enabled (hover near corners to reveal controls)",
            "歌詞没入モードをオンにしました（四隅にカーソルを近づけると表示）",
            "가사 몰입 모드가 켜졌습니다 (모서리에 마우스를 대면 컨트롤 표시)"
        ]),
        ("已关闭歌词沉浸模式", ["已關閉歌詞沉浸模式", "Immersive lyrics disabled", "歌詞没入モードをオフにしました", "가사 몰입 모드가 꺼졌습니다"]),
        ("已载入本地同步歌词文件", ["已載入本機同步歌詞檔案", "Loaded local synced lyrics", "ローカル同期歌詞を読み込みました", "로컬 싱크 가사를 불러왔습니다"]),
        ("已从剪贴板载入同步歌词", ["已從剪貼簿載入同步歌詞", "Loaded synced lyrics from clipboard", "クリップボードから歌詞を読み込みました", "클립보드에서 싱크 가사를 불러왔습니다"]),
        ("未登录网易云账号（游客模式可浏览榜单并播放免费音源）", [
            "未登入網易雲帳號（訪客模式可瀏覽榜單並播放免費音源）",
            "Not signed in (Guest mode can browse charts & play free tracks)",
            "未ログイン（ゲストモードでチャート閲覧・無料曲再生が可能）",
            "로그인 안 됨 (게스트 모드로 차트 탐색 및 무료 곡 재생 가능)"
        ]),
        ("未配置 QQ 音乐 Cookie（游客模式可浏览全站并播放免费曲目）", [
            "未設定 QQ 音樂 Cookie（訪客模式可瀏覽全站並播放免費曲目）",
            "Cookie not set (Guest mode can browse catalog & play free tracks)",
            "Cookie未設定（ゲストモードで全曲検索・無料曲再生が可能）",
            "쿠키 미설정 (게스트 모드로 탐색 및 무료 곡 재생 가능)"
        ]),
        ("尚未检测到活跃的外部媒体会话（在 Spotify / 网易云客户端 / QQ 音乐客户端 / 浏览器播放即可自动接管）", [
            "尚未偵測到活躍的外部媒體工作階段（在 Spotify / 網易雲 / QQ 音樂 / 瀏覽器播放即可自動接管）",
            "No active external media session detected (play audio in Spotify, browser, or any app to link automatically)",
            "アクティブな外部メディアセッションが見つかりません（Spotifyやブラウザで再生すると自動連携）",
            "활성화된 외부 미디어 세션이 없습니다 (Spotify나 브라우저에서 재생하면 자동 연동됩니다)"
        ]),
        ("订阅有效（可直接播放）", ["訂閱有效（可直接播放）", "Active Subscription (Ready to play)", "サブスクリプション有効（再生可能）", "구독 활성 (재생 가능)"]),
        ("已读取登录", ["已讀取登入", "Signed In", "ログイン済", "로그인됨"]),
        ("已连接 Apple Music 账号：将自动逐首匹配曲库歌曲并新建 Apple Music 云端歌单。", [
            "已連結 Apple Music 帳號：將自動逐首配對曲庫歌曲並新建 Apple Music 雲端歌單。",
            "Apple Music connected: tracks will be matched one by one and saved to a new cloud playlist.",
            "Apple Music連携済み：1曲ずつ照合して新しいクラウドプレイリストを作成します。",
            "Apple Music 연결됨: 곡을 하나씩 매칭하여 새 클라우드 플레이리스트로 생성합니다."
        ]),
        ("已连接网易云音乐账号：将自动逐首匹配网易云曲库并新建云端歌单。", [
            "已連結網易雲音樂帳號：將自動逐首配對網易雲曲庫並新建雲端歌單。",
            "NetEase account connected: tracks will be matched and saved to a new NetEase cloud playlist.",
            "NetEase連携済み：1曲ずつ照合して新しいクラウドプレイリストを作成します。",
            "넷이즈 계정 연결됨: 곡을 하나씩 매칭하여 새 클라우드 플레이리스트로 생성합니다."
        ]),
        ("将在 QQ 音乐全库逐首匹配并生成可一键切换直出播放的跨平台队列。", [
            "將在 QQ 音樂全庫逐首配對並產生可一鍵切換直出播放的跨平台佇列。",
            "Matches tracks across QQ Music and builds a ready-to-play cross-platform queue.",
            "QQ Musicから1曲ずつ照合し、そのまま切替再生できるキューを生成します。",
            "QQ 뮤직에서 곡을 매칭하여 바로 전환 재생할 수 있는 큐를 생성합니다."
        ]),
        ("提示：尚未绑定 Apple Music Media-User-Token，请先前往「设置」绑定后再复制。", [
            "提示：尚未綁定 Apple Music Media-User-Token，請先前往「設定」綁定後再複製。",
            "Note: Bind your Apple Music Media-User-Token in Settings before transferring to Apple Music.",
            "ヒント：設定で Apple Music Media-User-Token を登録してから複製してください。",
            "안내: Apple Music으로 복사하려면 먼저 설정에서 Media-User-Token을 등록하세요."
        ]),
        ("提示：尚未登录网易云音乐账号，请先前往「设置」扫码或粘贴 Cookie 登录。", [
            "提示：尚未登入網易雲音樂帳號，請先前往「設定」掃碼或貼上 Cookie 登入。",
            "Note: Sign in to NetEase via QR or Cookie in Settings before transferring to NetEase.",
            "ヒント：設定で NetEase にQRまたはCookieログインしてから複製してください。",
            "안내: 넷이즈로 복사하려면 먼저 설정에서 QR 또는 쿠키로 로그인하세요."
        ]),
        ("若原版 Apple Music 仍遇到点歌秒切、0:00 无声，可点击「一键修复」重置设备身份（自动备份）", [
            "若原版 Apple Music 仍遇到點歌秒切、0:00 無聲，可點擊「一鍵修復」重設裝置身分（自動備份）",
            "If official Apple Music still skips or stalls at 0:00, click 1-Click Repair to reset device identity (auto-backed up)",
            "公式 Apple Music でスキップや 0:00 停止が続く場合、「ワンクリック修復」でデバイスIDをリセット（自動バックアップ）",
            "공식 Apple Music에서 계속 건너뛰거나 0:00에 멈추면 '원클릭 복구'로 기기 ID를 재설정하세요 (자동 백업)"
        ]),
        ("若原版 Apple Music 出现无法播放或秒切，可点击「一键修复」备份并清理重定向", [
            "若原版 Apple Music 出現無法播放或秒切，可點擊「一鍵修復」備份並清理重新導向",
            "If official Apple Music cannot play or skips tracks, click 1-Click Repair to back up and clear redirections",
            "公式 Apple Music が再生できない場合、「ワンクリック修復」でリダイレクトをバックアップ・解除してください",
            "공식 Apple Music이 재생되지 않거나 건너뛰면 '원클릭 복구'로 리디렉션을 백업 후 정리하세요"
        ]),
        ("已读取登录", ["已讀取登入", "Signed In", "ログイン済", "로그인됨"]),
        ("订阅有效（可直接播放）", ["訂閱有效（可直接播放）", "Active Subscription (Ready to Play)", "サブスクリプション有効（再生可）", "구독 활성 (재생 가능)"]),
        ("跟随系统", ["跟隨系統", "Auto (System)", "システムに従う", "시스템 자동"]),
        ("一起听", ["一起聽", "Listen Together", "一緒に聴く", "함께 듣기"]),
        ("网易云 · 一起听", ["網易雲 · 一起聽", "NetEase · Listen Together", "NetEase · 一緒に聴く", "넷이즈 · 함께 듣기"]),
        ("创建一起听房间", ["建立一起聽房間", "Create Room", "ルーム作成", "방 만들기"]),
        ("粘贴链接加入", ["貼上連結加入", "Paste Link to Join", "リンク貼付で参加", "링크 붙여넣기 참가"]),
        ("同步手机当前房间", ["同步手機目前房間", "Sync Active Room", "進行中ルーム同期", "진행 중인 방 동기화"]),
        ("私信一起听邀请", ["私訊一起聽邀請", "Inbox Invitations", "DM 招待一覧", "받은 초대 메시지"]),
        ("刷新私信", ["重新整理私訊", "Refresh Inbox", "DM 更新", "새로고침"]),
        ("加入房间", ["加入房間", "Join Room", "参加", "참가"]),
        ("复制邀请链接", ["複製邀請連結", "Copy Invite Link", "招待リンクコピー", "초대 링크 복사"]),
        ("同步房主歌单", ["同步房主歌單", "Sync Queue", "キュー同期", "재생목록 동기화"]),
        ("立即校准进度", ["立即校準進度", "Resync Now", "再生位置同期", "지금 동기화"]),
        ("退出 / 结束房间", ["退出 / 結束房間", "Leave / End Room", "退出 / 終了", "나가기 / 종료"]),
        ("前往设置登录", ["前往設定登入", "Sign In in Settings", "設定でログイン", "설정에서 로그인"]),
        ("使用网易云「一起听」需要先登录网易云账号（支持扫码或粘贴 Cookie）。", [
            "使用網易雲「一起聽」需要先登入網易雲帳號（支援掃碼或貼上 Cookie）。",
            "Sign in to your NetEase account (QR or Cookie in Settings) to use Listen Together.",
            "一緒に聴くを使うには NetEase アカウントのログイン（QRまたはCookie）が必要です。",
            "함께 듣기를 사용하려면 설정에서 넷이즈 계정에 로그인하세요."
        ]),
        ("提示：复制好友发来的网易云一起听链接后，直接按 Ctrl+V 或点击「粘贴链接加入」即可与手机端同步听歌。", [
            "提示：複製好友發來的網易雲一起聽連結後，直接按 Ctrl+V 或點擊「貼上連結加入」即可與手機端同步聽歌。",
            "Tip: Copy a friend's NetEase Listen Together link and press Ctrl+V or click Paste Link to sync with their phone.",
            "ヒント：友達の一緒に聴くリンクをコピーして Ctrl+V または「リンク貼付で参加」でスマホと同期再生できます。",
            "팁: 친구의 함께 듣기 링크를 복사한 뒤 Ctrl+V 또는 링크 붙여넣기 참가를 눌러 동기화하세요."
        ]),
        ("暂无私信邀请 · 可点击「创建一起听房间」生成邀请链接发给好友", [
            "暫無私訊邀請 · 可點擊「建立一起聽房間」產生邀請連結發給好友",
            "No inbox invites yet — click Create Room to generate a share link for friends",
            "DM 招待はありません。「ルーム作成」で友達への招待リンクを作成できます",
            "받은 초대가 없습니다. '방 만들기'로 초대 링크를 생성해 친구에게 공유하세요."
        ]),
        ("等待好友通过邀请链接加入房间...", [
            "等待好友透過邀請連結加入房間...",
            "Waiting for friends to join via invite link...",
            "招待リンクからの友達の参加を待機中...",
            "초대 링크로 친구가 입장하길 기다리는 중..."
        ]),
        ("在 AM Light 任意播放一首网易云歌曲，将自动同步到一起听房间队列。", [
            "在 AM Light 任意播放一首網易雲歌曲，將自動同步到一起聽房間佇列。",
            "Play any NetEase song in AM Light to sync it automatically with the room.",
            "AM Light で任意の NetEase 楽曲を再生するとルームに自動同期されます。",
            "AM Light에서 넷이즈 곡을 재생하면 방 대기열에 자동 동기화됩니다."
        ]),
        ("已复制一起听邀请链接，发给好友用网易云打开即可加入", [
            "已複製一起聽邀請連結，發給好友用網易雲開啟即可加入",
            "Invite link copied — send it to friends to join from the NetEase app",
            "招待リンクをコピーしました。友達に送って参加してもらいましょう",
            "초대 링크를 복사했습니다. 친구에게 보내 함께 들어보세요"
        ]),
        ("对方已结束网易云一起听房间", [
            "對方已結束網易雲一起聽房間",
            "Partner ended the Listen Together room",
            "相手が一緒に聴くルームを終了しました",
            "상대방이 함께 듣기 방을 종료했습니다"
        ]),
        ("已退出网易云一起听房间", [
            "已退出網易雲一起聽房間",
            "Left the Listen Together room",
            "一緒に聴くルームを退出しました",
            "함께 듣기 방에서 나갔습니다"
        ]),
        ("粘贴口令/链接加入", ["貼上口令/連結加入", "Paste Code / Link", "コード・リンク貼付", "초대 링크 붙여넣기"]),
        ("创建房间与邀请互关", ["建立房間與邀請互關", "Room & Mutuals", "ルーム・相互フォロー", "방 만들기 · 맞팔 초대"]),
        ("房间控制与互关邀请", ["房間控制與互關邀請", "Room & Mutuals", "ルーム管理・招待", "방 제어 · 맞팔 초대"]),
        ("私信邀请", ["私訊邀請", "Inbox Invites", "DM 招待", "받은 초대"]),
        ("刷新互关", ["重新整理互關", "Refresh Friends", "友達更新", "맞팔 새로고침"]),
        ("邀请一起听", ["邀請一起聽", "Invite", "一緒に聴く招待", "함께 듣기 초대"]),
        ("再次邀请", ["再次邀請", "Re-invite", "再招待", "다시 초대"]),
        ("邀请", ["邀請", "Invite", "招待", "초대"]),
        ("已邀请", ["已邀請", "Invited", "招待済", "초대됨"]),
        ("互关好友", ["互關好友", "Mutual Friend", "相互フォロー", "맞팔 친구"]),
        ("互关 · VIP", ["互關 · VIP", "Mutual · VIP", "相互 · VIP", "맞팔 · VIP"]),
        ("关注 · VIP", ["關注 · VIP", "Following · VIP", "フォロー · VIP", "팔로잉 · VIP"]),
        ("关注好友", ["關注好友", "Following", "フォロー中", "팔로잉"]),
        ("黑胶 VIP", ["黑膠 VIP", "Vinyl VIP", "ブラックVIP", "바이닐 VIP"]),
        ("黑胶 SVIP", ["黑膠 SVIP", "Vinyl SVIP", "ブラックSVIP", "바이닐 SVIP"]),
        ("音乐包", ["音樂包", "Music Pack", "ミュージックパック", "뮤직 팩"]),
        ("普通用户", ["普通使用者", "Free User", "一般ユーザー", "일반 사용자"]),
        ("非 VIP", ["非 VIP", "Non-VIP", "非 VIP", "비 VIP"]),
        ("播放进度已与对方同步", ["播放進度已與對方同步", "Playback is synced with partner", "再生位置を相手と同期しました", "재생 위치가 상대방과 동기화되었습니다"]),
        ("已将进度校准至对方当前位置", ["已將進度校準至對方目前位置", "Synced playhead to partner", "相手の再生位置に合わせました", "상대방 재생 위치로 동기화했습니다"]),
        ("正在解析网易云一起听短链...", ["正在解析網易雲一起聽短鏈...", "Resolving NetEase short link...", "短縮リンクを解析中...", "단축 링크 확인 중..."]),
        ("支持网易云手机口令短链（如 https://163cn.tv/...）、完整分享链接或房间号，可直接按 Ctrl+V 加入。", [
            "支援網易雲手機口令短鏈（如 https://163cn.tv/...）、完整分享連結或房間號，可直接按 Ctrl+V 加入。",
            "Supports NetEase mobile short links (https://163cn.tv/...), full invite URLs, or room IDs. Press Ctrl+V to join.",
            "スマホの短縮リンク (https://163cn.tv/...) や招待URL、ルームIDに対応。Ctrl+V ですぐ参加できます。",
            "모바일 단축 링크(https://163cn.tv/...), 초대 URL, 방 ID를 지원합니다. Ctrl+V로 바로 참가하세요."
        ]),

        // --- Lyrics Menu, Queue, Liquid Glass v2 & Page Cache ---
        ("待播清单", ["待播清單", "Up Next", "再生キュー", "재생 대기열"]),
        ("左侧放大显示", ["左側放大顯示", "Enlarged Left Player", "左側プレイヤー拡大", "왼쪽 플레이어 확대"]),
        ("歌词中轴居中", ["歌詞中軸置中", "Center-Axis Lyrics", "歌詞を中央軸に配置", "가사 중앙축 정렬"]),
        ("歌词沉浸模式", ["歌詞沉浸模式", "Immersive Lyrics", "歌詞没入モード", "가사 몰입 모드"]),
        ("歌词偏移", ["歌詞偏移", "Lyrics Offset", "歌詞オフセット", "가사 오프셋"]),
        ("背景风格", ["背景風格", "Backdrop Style", "背景スタイル", "배경 스타일"]),
        ("背景风格：流光", ["背景風格：流光", "Backdrop: Aurora", "背景スタイル：オーロラ", "배경 스타일: 오로라"]),
        ("背景风格：柔焦", ["背景風格：柔焦", "Backdrop: Soft Focus", "背景スタイル：ソフト", "배경 스타일: 소프트"]),
        ("歌词来源", ["歌詞來源", "Lyrics Source", "歌詞ソース", "가사 소스"]),
        ("页面缓存", ["頁面快取", "Page Cache", "ページキャッシュ", "페이지 캐시"]),
        ("页面缓存与加载加速", ["頁面快取與載入加速", "Page Cache & Fast Navigation", "ページキャッシュ・高速切替", "페이지 캐시 및 빠른 전환"]),
        ("开启后自动预热侧栏并秒开歌单/专辑详情（后台静默更新）；关闭则每次重新拉取", [
            "開啟後自動預熱側欄並秒開歌單/專輯詳情（背景靜默更新）；關閉則每次重新拉取",
            "Pre-warms sidebar tabs and opens playlists/albums instantly with silent background revalidation",
            "サイドバーを事前読込し、プレイリストやアルバムを瞬時表示（裏で自動更新）",
            "사이드바를 미리 불러오고 플레이리스트·앨범을 즉시 표시합니다 (백그라운드 자동 갱신)"
        ]),
        ("刷新与清空页面缓存", ["重新整理與清空頁面快取", "Refresh & Clear Page Cache", "更新とキャッシュ消去", "새로고침 및 캐시 지우기"]),
        ("刷新当前页", ["重新整理目前頁", "Refresh Page", "現在のページを更新", "현재 페이지 새로고침"]),
        ("清空缓存", ["清空快取", "Clear Cache", "キャッシュ消去", "캐시 지우기"]),
        ("已清空页面缓存，下次访问将重新拉取最新数据", [
            "已清空頁面快取，下次造訪將重新拉取最新資料",
            "Page cache cleared — fresh data will be fetched on next visit",
            "ページキャッシュを消去しました。次回表示時に最新データを取得します",
            "페이지 캐시를 지웠습니다. 다음 방문 시 최신 데이터를 불러옵니다"
        ]),
        ("正在刷新当前页面数据...", [
            "正在重新整理目前頁面資料...",
            "Refreshing current view...",
            "現在のページを更新中...",
            "현재 페이지 데이터를 새로고침하는 중..."
        ]),
        ("默认开启；侧栏与浮层启用 iOS 26 光学折射液态玻璃材质", [
            "預設開啟；側欄與浮層啟用 iOS 26 光學折射液態玻璃材質",
            "On by default; applies iOS 26 optical refractive Liquid Glass to sidebar & floating controls",
            "既定オン。サイドバーと浮遊UIに iOS 26 光学屈折リキッドガラスを適用",
            "기본 켜짐. 사이드바와 플로팅 UI에 iOS 26 광학 굴절 리퀴드 글래스를 적용합니다"
        ]),
        ("未能载入内容，请按 F5 或前往设置刷新重试。", [
            "未能載入內容，請按 F5 或前往設定重新整理重試。",
            "Failed to load content. Press F5 or refresh from Settings to retry.",
            "読み込みに失敗しました。F5 キーまたは設定から更新して再試行してください。",
            "콘텐츠를 불러오지 못했습니다. F5 키를 누르거나 설정에서 새로고침하세요."
        ]),
        ("暂无内容", ["暫無內容", "No items available", "項目がありません", "항목이 없습니다"]),

        // --- iPadOS Flat Dark Settings Page (Headers, Labels & Footers) ---
        ("音频", ["音訊", "Audio", "オーディオ", "오디오"]),
        ("外观与显示", ["外觀與顯示", "Appearance & Display", "外観と表示", "화면 및 표시"]),
        ("性能与缓存", ["效能與快取", "Performance & Cache", "パフォーマンスとキャッシュ", "성능 및 캐시"]),
        ("Hi-Res 24/192", ["Hi-Res 24/192", "Hi-Res 24/192", "Hi-Res 24/192", "Hi-Res 24/192"]),
        ("无损 24/48", ["無損 24/48", "Lossless 24/48", "ロスレス 24/48", "무손실 24/48"]),
        ("已展开", ["已展開", "Expanded", "展開中", "펼쳐짐"]),
        ("打开日志目录", ["開啟日誌目錄", "Open Logs Folder", "ログフォルダを開く", "로그 폴더 열기"]),
        ("存在崩溃记录", ["存在當機記錄", "Crash Log Found", "クラッシュ記録あり", "충돌 기록 있음"]),
        ("运行与崩溃日志", ["執行與當機日誌", "Runtime & Crash Logs", "実行・クラッシュログ", "실행 및 충돌 로그"]),
        ("系统媒体控制发布 (SMTC)", [
            "系統媒體控制發佈 (SMTC)",
            "System Media Controls (SMTC) Publishing",
            "システムメディア制御 (SMTC) 公開",
            "시스템 미디어 제어 (SMTC) 게시"
        ]),
        ("更改音频质量后将在下一首播放或重新缓冲时生效；开启 SMTC 发布可联动系统媒体卡片、耳机播控与 Lyricify 等外部歌词工具。", [
            "更改音訊品質後將在下一首播放或重新緩衝時生效；開啟 SMTC 發佈可連動系統媒體卡片、耳機播控與 Lyricify 等外部歌詞工具。",
            "Audio quality changes apply on next track or rebuffer; SMTC publishing enables Windows media controls, headset keys, and Lyricify.",
            "音質設定は次曲から適用。SMTC公開をオンにするとシステムメディア制御・イヤホン操作・Lyricify等と連携します。",
            "음질 변경은 다음 곡부터 적용됩니다. SMTC 게시를 켜면 시스템 미디어 카드·헤드셋 제어·Lyricify 등과 연동됩니다."
        ]),
        ("更改音频质量后将在下一首播放或重新缓冲时生效。", [
            "更改音訊品質後將在下一首播放或重新緩衝時生效。",
            "Audio quality changes apply on the next track or when rebuffering.",
            "音質の変更は次の曲の再生または再バッファリング時に適用されます。",
            "오디오 음질 변경은 다음 곡 재생 또는 재버퍼링 시 적용됩니다."
        ]),
        ("液态玻璃默认开启，作用于侧栏与浮层控件；设置页始终使用纯黑静态界面以节省 GPU 资源。", [
            "液態玻璃預設開啟，作用於側欄與浮層控制項；設定頁始終使用純黑靜態介面以節省 GPU 資源。",
            "Liquid Glass is on by default for the sidebar and floating chrome. Settings stays flat and static to save GPU power.",
            "リキッドガラスはサイドバーや浮遊コントロールで既定オンです。設定画面は省電力のため常にフラット表示になります。",
            "리퀴드 글래스는 사이드바와 플로팅 UI에 기본 적용되며, 설정 화면은 GPU 절약을 위해 항상 평면 정적 UI를 유지합니다."
        ]),
        ("开启后自动预热侧栏并秒开歌单/专辑详情（后台静默更新）；关闭则每次重新拉取。", [
            "開啟後自動預熱側欄並秒開歌單/專輯詳情（背景靜默更新）；關閉則每次重新拉取。",
            "Pre-warms sidebar tabs and opens playlists/albums instantly with silent background revalidation.",
            "サイドバーを事前読込し、プレイリストやアルバムを瞬時表示（裏で自動更新）します。",
            "사이드바를 미리 불러오고 플레이리스트·앨범을 즉시 표시합니다 (백그라운드 자동 갱신)."
        ]),
        ("沉浸模式隐藏歌词页四角控件并在鼠标靠近时自动渐显；开启降采样可将未唱行景深降至 0.75× 节能。", [
            "沉浸模式隱藏歌詞頁四角控制項並在滑鼠靠近時自動漸顯；開啟降採樣可將未唱行景深降至 0.75× 節能。",
            "Immersive mode hides corner controls until hovered; downsample lowers inactive line blur to 0.75× to save power.",
            "没入モードは四隅の操作UIを隠しホバー時に表示。ダウンサンプリングで未唱行のぼかしを 0.75× に省電力化します。",
            "몰입 모드는 모서리 컨트롤을 숨기고 마우스를 가까이 대면 표시합니다. 다운샘플링으로 비활성 가사 블러를 0.75×로 절전합니다."
        ]),
        ("仅在主页与歌词页启用纯 Win32 硬件解码动态封面；关闭实时频谱播放图标可停用后台采样。", [
            "僅在主頁與歌詞頁啟用純 Win32 硬體解碼動態封面；關閉即時頻譜播放圖示可停用背景取樣。",
            "Hardware-decoded motion covers run on Home and Lyrics only; turning off live spectrum icon stops background audio sampling.",
            "ハードウェア再生の動くアートワークはホームと歌詞画面のみ有効。リアルタイム指標オフで裏の音声サンプリングを停止します。",
            "하드웨어 디코딩 모션 아트워크는 홈과 가사 화면에서만 동작하며, 실시간 스펙트럼 아이콘을 끄면 백그라운드 샘플링을 중단합니다."
        ]),
        ("外化头影与早期反射渲染（需两路可用输出端点）", [
            "外化頭影與早期反射算繪（需兩路可用輸出端點）",
            "Externalized head-shadow & early reflections (requires 2 output endpoints)",
            "頭外定位と初期反射レンダリング（2系統の出力エンドポイントが必要）",
            "외부화 두부 그림자 및 초기 반사 렌더링 (출력 엔드포인트 2개 필요)"
        ]),
    ];

    let mut map = HashMap::with_capacity(entries.len() * 2);
    for &(k, v) in entries {
        map.insert(k, v);
    }
    map
});

fn pick(lang: ResolvedLang, choices: [&'static str; 4]) -> &'static str {
    match lang.idx() {
        Some(i) => choices[i],
        None => choices[0],
    }
}

fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

/// Translates known AM Light UI strings and parameterized UI templates into the active language.
/// Zero-cost (`Cow::Borrowed`) when language is Simplified Chinese or when text contains no CJK
/// or is user/media metadata (song titles, artist names, album titles, lyrics) not in the UI table.
pub fn translate_ui_text<'a>(input: &'a str) -> Cow<'a, str> {
    let lang = active_resolved_language();
    translate_ui_text_for(input, lang)
}

pub fn translate_ui_text_for<'a>(input: &'a str, lang: ResolvedLang) -> Cow<'a, str> {
    if lang == ResolvedLang::ZhCn || input.is_empty() || !contains_cjk(input) {
        return Cow::Borrowed(input);
    }

    if let Some(&vals) = EXACT_TABLE.get(input) {
        return Cow::Borrowed(pick(lang, vals));
    }

    // Shelf headings with trailing ` ›`
    if let Some(stem) = input.strip_suffix(" ›") {
        if let Some(&vals) = EXACT_TABLE.get(stem) {
            return Cow::Owned(format!("{} ›", pick(lang, vals)));
        }
    }

    // O(1) first-char fast reject for media metadata (CJK song titles, artists, albums, lyrics)
    // before scanning parameterized UI prefixes (~10x faster on En/Ja/Ko/ZhTw locales!).
    let has_mid_dot = input.contains(" · ");
    let may_match_prefix = matches!(
        input.chars().next(),
        Some(
            '待' | '偏' | '一' | '房' | '主' | '邀' | '私' | '黑' | '实' | '播' | '前'
                | '声' | 'F' | '自' | '垂' | '当' | '源' | '正' | 'A' | '个' | '“'
                | '词' | '静' | '已' | '界' | '歌' | '背' | '艺' | 'D' | '检'
        )
    );
    if !may_match_prefix && !has_mid_dot {
        return Cow::Borrowed(input);
    }

    // Parameterized AM Light UI format templates (strictly preserving track/artist/playlist names!)
    if let Some(rest) = input.strip_prefix("待播清单 (") {
        let p = pick(lang, ["待播清單 (", "Up Next (", "再生キュー (", "재생 대기열 ("]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("偏好设置 (") {
        let p = pick(lang, ["偏好設定 (", "Preferences (", "環境設定 (", "환경설정 ("]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("一起听 · ") {
        let p = pick(lang, ["一起聽 · ", "Together · ", "一緒に聴く · ", "함께 듣기 · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("房间号：") {
        let p = pick(lang, ["房間號：", "Room ID: ", "ルームID：", "방 ID: "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("房间共享队列 (") {
        let p = pick(lang, ["房間共享佇列 (", "Shared Queue (", "共有キュー (", "공유 대기열 ("]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("主动邀请互关好友 (") {
        let p = pick(lang, ["主動邀請互關好友 (", "Invite Mutual Friends (", "相互フォローを招待 (", "맞팔 친구 초대 ("]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("邀请互关好友加入当前房间 (") {
        let p = pick(lang, ["邀請互關好友加入目前房間 (", "Invite Friends to Room (", "友達をルームに招待 (", "현재 방에 맞팔 초대 ("]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("私信邀请 (") {
        let p = pick(lang, ["私訊邀請 (", "Inbox Invites (", "DM 招待 (", "받은 초대 ("]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("黑胶 SVIP Lv.") {
        let p = pick(lang, ["黑膠 SVIP Lv.", "SVIP Lv.", "SVIP Lv.", "SVIP Lv."]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("黑胶 VIP Lv.") {
        let p = pick(lang, ["黑膠 VIP Lv.", "VIP Lv.", "VIP Lv.", "VIP Lv."]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("实时双向同步中 · ") {
        let p = pick(lang, ["即時雙向同步中 · ", "Live Sync Active · ", "双方向同期中 · ", "실시간 동기화 중 · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("播放音量  ") {
        let p = pick(lang, ["播放音量  ", "Volume  ", "音量  ", "음량  "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("前往艺人 · ") {
        let p = pick(lang, ["前往藝人 · ", "Go to Artist · ", "アーティストへ · ", "아티스트로 이동 · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("前往专辑 · ") {
        let p = pick(lang, ["前往專輯 · ", "Go to Album · ", "アルバムへ · ", "앨범으로 이동 · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("声场宽度  ") {
        let p = pick(lang, ["聲場寬度  ", "Stage Width  ", "音場幅  ", "음장 폭  "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("房间混响  ") {
        let p = pick(lang, ["房間混響  ", "Room Ambience  ", "リバーブ  ", "공간 잔향  "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("FFT 频谱 · ") {
        let p = pick(lang, ["FFT 頻譜 · ", "FFT Spectrum · ", "FFT スペクトラム · ", "FFT 스펙트럼 · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("声谱瀑布 · ") {
        let p = pick(lang, ["聲譜瀑布 · ", "Spectrogram · ", "スペクトログラム · ", "스펙트로그램 · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("实测 · 置信度 ") {
        let p = pick(lang, ["實測 · 信心度 ", "Measured · Confidence ", "実測 · 信頼度 ", "실측 · 신뢰도 "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("实测 · ") {
        let p = pick(lang, ["實測 · ", "Measured · ", "実測 · ", "실측 · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("自适应 ") {
        let p = pick(lang, ["自適應 ", "Adaptive ", "自動 ", "자동 "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("垂直同步 ") {
        let p = pick(lang, ["垂直同步 ", "VSync ", "VSync ", "수직동기화 "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("自定义 ") {
        let p = pick(lang, ["自訂 ", "Custom ", "カスタム ", "사용자 지정 "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("当前屏幕 ") {
        if let Some((hz, tgt)) = rest.split_once(" · 目标刷新率：") {
            let tgt_tr = translate_ui_text_for(tgt, lang);
            return Cow::Owned(match lang {
                ResolvedLang::ZhTw => format!("目前螢幕 {hz} · 目標更新率：{tgt_tr}"),
                ResolvedLang::En => format!("Display {hz} · Target: {tgt_tr}"),
                ResolvedLang::Ja => format!("現在の画面 {hz} · 目標: {tgt_tr}"),
                ResolvedLang::Ko => format!("현재 디스플레이 {hz} · 목표: {tgt_tr}"),
                ResolvedLang::ZhCn => input.to_string(),
            });
        }
    }
    if let Some(rest) = input.strip_prefix("当前 ") {
        if let Some((px, _)) = rest.split_once(" · 可选直角或拖动滑杆") {
            return Cow::Owned(match lang {
                ResolvedLang::ZhTw => format!("目前 {px} · 可選直角或拖動滑桿精細調節圓角大小"),
                ResolvedLang::En => format!("Current {px} · Pick a preset or drag slider to fine-tune corners"),
                ResolvedLang::Ja => format!("現在 {px} · プリセット選択またはスライダーで角丸を調整"),
                ResolvedLang::Ko => format!("현재 {px} · 프리셋 선택 또는 슬라이더로 모서리 조절"),
                ResolvedLang::ZhCn => input.to_string(),
            });
        }
    }
    if let Some(rest) = input.strip_prefix("源歌单：「") {
        if let Some((title, tail)) = rest.split_once("」 · 共 ") {
            let num = tail.trim_end_matches(" 首歌曲");
            return Cow::Owned(match lang {
                ResolvedLang::ZhTw => format!("來源歌單：「{title}」 · 共 {num} 首歌曲"),
                ResolvedLang::En => format!("Source: \"{title}\" · {num} tracks"),
                ResolvedLang::Ja => format!("元プレイリスト：「{title}」・全 {num} 曲"),
                ResolvedLang::Ko => format!("원본: \"{title}\" · 총 {num}곡"),
                ResolvedLang::ZhCn => input.to_string(),
            });
        }
    }
    if let Some(rest) = input.strip_prefix("正在匹配歌曲... 已命中 ") {
        return Cow::Owned(match lang {
            ResolvedLang::ZhTw => format!("正在配對歌曲... 已命中 {rest}"),
            ResolvedLang::En => format!("Matching tracks... Matched {rest}"),
            ResolvedLang::Ja => format!("楽曲を照合中... 一致 {rest}"),
            ResolvedLang::Ko => format!("곡 매칭 중... 일치 {rest}"),
            ResolvedLang::ZhCn => input.to_string(),
        });
    }
    if let Some(rest) = input.strip_prefix("AM Light · ") {
        let sub = translate_ui_text_for(rest, lang);
        return Cow::Owned(format!("AM Light · {sub}"));
    }
    if let Some(rest) = input.strip_prefix("个人资料库授权：已持久保存 (") {
        let inner = rest.trim_end_matches(')').trim_end_matches(" 字符");
        return Cow::Owned(match lang {
            ResolvedLang::ZhTw => format!("個人資料庫授權：已持久儲存 ({inner} 字元)"),
            ResolvedLang::En => format!("Library Token: Saved ({inner} chars)"),
            ResolvedLang::Ja => format!("ライブラリToken：保存済み ({inner} 文字)"),
            ResolvedLang::Ko => format!("보관함 토큰: 저장됨 ({inner}자)"),
            ResolvedLang::ZhCn => input.to_string(),
        });
    }
    if let Some(rest) = input.strip_prefix("“") {
        if let Some(q) = rest.strip_suffix("” 的搜索结果") {
            return Cow::Owned(match lang {
                ResolvedLang::ZhTw => format!("「{q}」的搜尋結果"),
                ResolvedLang::En => format!("Search results for \"{q}\""),
                ResolvedLang::Ja => format!("「{q}」の検索結果"),
                ResolvedLang::Ko => format!("\"{q}\" 검색 결과"),
                ResolvedLang::ZhCn => input.to_string(),
            });
        }
    }
    if let Some(rest) = input.strip_prefix("词曲作者：") {
        let p = pick(lang, ["詞曲作者：", "Songwriters: ", "作詞・作曲：", "작사·작곡: "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("正在准备播放… · ") {
        let p = pick(lang, ["正在準備播放… · ", "Preparing playback... · ", "再生準備中… · ", "재생 준비 중… · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("静默待机 · ") {
        let p = pick(lang, ["靜默待機 · ", "Idle Standby · ", "アイドル待機 · ", "절전 대기 · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("当前接管：") {
        let p = pick(lang, ["目前接管：", "Connected: ", "連携中：", "연동 중: "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("已保存登录：") {
        let p = pick(lang, ["已儲存登入：", "Signed in: ", "ログイン済み：", "로그인됨: "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("已保存连接：") {
        let p = pick(lang, ["已儲存連線：", "Connected: ", "接続済み：", "연결됨: "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("已切换播放后端：") {
        let p = pick(lang, ["已切換播放後端：", "Switched playback backend: ", "再生バックエンドを切替：", "재생 백엔드 전환: "]);
        let tail = translate_ui_text_for(rest, lang);
        return Cow::Owned(format!("{p}{tail}"));
    }
    if let Some(rest) = input.strip_prefix("界面语言已切换为：") {
        let p = pick(lang, ["介面語言已切換為：", "Language switched to: ", "表示言語を切り替えました：", "언어가 변경되었습니다: "]);
        let tail = translate_ui_text_for(rest, lang);
        return Cow::Owned(format!("{p}{tail}"));
    }
    if let Some(rest) = input.strip_prefix("歌词来源已切换为：") {
        let p = pick(lang, ["歌詞來源已切換為：", "Lyrics source set to: ", "歌詞ソースを切替：", "가사 소스 변경: "]);
        let tail = translate_ui_text_for(rest, lang);
        return Cow::Owned(format!("{p}{tail}"));
    }
    if let Some(rest) = input.strip_prefix("歌词来源：") {
        let p = pick(lang, ["歌詞來源：", "Lyrics Source: ", "歌詞ソース：", "가사 소스: "]);
        let tail = translate_ui_text_for(rest, lang);
        return Cow::Owned(format!("{p}{tail}"));
    }
    if let Some(rest) = input.strip_prefix("背景风格：") {
        let p = pick(lang, ["背景風格：", "Backdrop: ", "背景スタイル：", "배경 스타일: "]);
        let tail = translate_ui_text_for(rest, lang);
        return Cow::Owned(format!("{p}{tail}"));
    }
    if let Some(rest) = input.strip_prefix("已缓存 ") {
        if let Some((cnt, _)) = rest.split_once(" 个详情页") {
            return Cow::Owned(match lang {
                ResolvedLang::ZhTw => format!("已快取 {cnt} 個詳情頁 · 按 F5 可隨時強制重新整理目前頁面"),
                ResolvedLang::En => format!("{cnt} detail pages cached · Press F5 anytime to force refresh"),
                ResolvedLang::Ja => format!("{cnt} 件の詳細をキャッシュ済 · F5 キーでいつでも強制更新"),
                ResolvedLang::Ko => format!("상세 페이지 {cnt}개 캐시됨 · F5 키로 언제든 강제 새로고침"),
                ResolvedLang::ZhCn => input.to_string(),
            });
        }
        if let Some((cnt, _)) = rest.split_once(" 项") {
            return Cow::Owned(match lang {
                ResolvedLang::ZhTw => format!("已快取 {cnt} 項"),
                ResolvedLang::En => format!("{cnt} cached"),
                ResolvedLang::Ja => format!("{cnt} 件キャッシュ済"),
                ResolvedLang::Ko => format!("{cnt}개 캐시됨"),
                ResolvedLang::ZhCn => input.to_string(),
            });
        }
    }
    if let Some(rest) = input.strip_prefix("艺人 · ") {
        let p = pick(lang, ["藝人 · ", "Artist · ", "アーティスト · ", "아티스트 · "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("正在打开艺人页：") {
        let p = pick(lang, ["正在開啟藝人頁：", "Opening artist: ", "アーティストページを開く：", "아티스트 페이지 열기: "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("正在打开专辑：") {
        let p = pick(lang, ["正在開啟專輯：", "Opening album: ", "アルバムを開く：", "앨범 열기: "]);
        return Cow::Owned(format!("{p}{rest}"));
    }
    if let Some(rest) = input.strip_prefix("已将「") {
        if let Some((tname, _)) = rest.split_once("」加入下一首播放") {
            return Cow::Owned(match lang {
                ResolvedLang::ZhTw => format!("已將「{tname}」加入下一首播放"),
                ResolvedLang::En => format!("Queued \"{tname}\" to Play Next"),
                ResolvedLang::Ja => format!("「{tname}」を次に再生へ追加しました"),
                ResolvedLang::Ko => format!("\"{tname}\"을(를) 다음 재생에 추가했습니다"),
                ResolvedLang::ZhCn => input.to_string(),
            });
        }
    }
    if let Some(rest) = input.strip_prefix("DRM 授权状态正常") {
        return Cow::Owned(match lang {
            ResolvedLang::ZhTw => format!("DRM 授權狀態正常{rest}"),
            ResolvedLang::En => "DRM authorization healthy (ADI identity files writable)".to_string(),
            ResolvedLang::Ja => "DRM認証ステータス正常（ADIファイル書込可能）".to_string(),
            ResolvedLang::Ko => "DRM 인증 상태 정상 (ADI 파일 쓰기 가능)".to_string(),
            ResolvedLang::ZhCn => input.to_string(),
        });
    }
    if let Some(rest) = input.strip_prefix("DRM 目录状态正常") {
        return Cow::Owned(match lang {
            ResolvedLang::ZhTw => format!("DRM 目錄狀態正常{rest}"),
            ResolvedLang::En => "DRM folder healthy (identity files initialize on first official playback)".to_string(),
            ResolvedLang::Ja => "DRMフォルダ正常（初回再生時に自動初期化）".to_string(),
            ResolvedLang::Ko => "DRM 폴더 상태 정상 (첫 재생 시 자동 초기화)".to_string(),
            ResolvedLang::ZhCn => input.to_string(),
        });
    }
    if let Some(rest) = input.strip_prefix("检测到 DRM 身份文件不可写") {
        return Cow::Owned(match lang {
            ResolvedLang::ZhTw => format!("偵測到 DRM 身分檔案不可寫{rest}"),
            ResolvedLang::En => "Read-only DRM identity files detected — click 1-Click Repair to fix".to_string(),
            ResolvedLang::Ja => "書込不可のDRMファイルを検出 — ワンクリック修復を実行してください".to_string(),
            ResolvedLang::Ko => "쓰기 불가 DRM 파일 감지됨 — 원클릭 복구를 실행하세요".to_string(),
            ResolvedLang::ZhCn => input.to_string(),
        });
    }
    if input.contains(" · ") {
        // Translate known UI-only composite quality/channel badges (e.g. "Hi-Res 无损 · 杜比全景声")
        let parts: Vec<&str> = input.split(" · ").collect();
        let mut any_hit = false;
        let mapped: Vec<Cow<'_, str>> = parts
            .iter()
            .map(|p| {
                if let Some(&vals) = EXACT_TABLE.get(*p) {
                    any_hit = true;
                    Cow::Borrowed(pick(lang, vals))
                } else {
                    Cow::Borrowed(*p)
                }
            })
            .collect();
        if any_hit {
            return Cow::Owned(mapped.join(" · "));
        }
    }

    Cow::Borrowed(input)
}
