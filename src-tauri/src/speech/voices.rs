//! Which voice reads which language, per engine.
//!
//! Keyed by the language names the rest of the app uses (`config::COMMON_LANGUAGES`).
//! A language typed by hand that is not listed still works: each engine has a
//! multilingual default that works the language out from the text.

/// MiniMax: (voice_id, language_boost). The voice ids come from `/v1/get_voice`; the
/// boost tells the model which language to pronounce, so even a voice from another
/// language reads correctly.
pub fn minimax(language: Option<&str>) -> (&'static str, &'static str) {
    match language.map(normalise).as_deref() {
        Some("english") => ("English_Graceful_Lady", "English"),
        Some("vietnamese") => ("Vietnamese_kindhearted_girl", "Vietnamese"),
        Some("simplified chinese" | "traditional chinese" | "chinese") => {
            ("Chinese (Mandarin)_News_Anchor", "Chinese")
        }
        Some("cantonese") => ("Cantonese_GentleLady", "Chinese,Yue"),
        Some("japanese") => ("Japanese_DependableWoman", "Japanese"),
        Some("korean") => ("Korean_CalmLady", "Korean"),
        Some("thai") => ("Thai_female_1_sample1", "Thai"),
        Some("french") => ("French_FemaleAnchor", "French"),
        Some("german") => ("German_SweetLady", "German"),
        Some("spanish") => ("Spanish_SereneWoman", "Spanish"),
        Some("portuguese") => ("Portuguese_Wiselady", "Portuguese"),
        Some("italian") => ("Italian_Narrator", "Italian"),
        Some("russian") => ("Russian_BrightHeroine", "Russian"),
        Some("arabic") => ("Arabic_CalmWoman", "Arabic"),
        Some("hindi") => ("hindi_female_1_v2", "Hindi"),
        Some("indonesian") => ("Indonesian_CalmWoman", "Indonesian"),
        _ => ("English_Graceful_Lady", "auto"),
    }
}

/// Azure: (voice name, xml:lang). The fallback is a multilingual neural voice, which
/// detects the language of the text it is given.
pub fn azure(language: Option<&str>) -> (&'static str, &'static str) {
    match language.map(normalise).as_deref() {
        Some("english") => ("en-US-AvaMultilingualNeural", "en-US"),
        Some("vietnamese") => ("vi-VN-HoaiMyNeural", "vi-VN"),
        Some("simplified chinese" | "chinese") => ("zh-CN-XiaoxiaoNeural", "zh-CN"),
        Some("traditional chinese") => ("zh-TW-HsiaoChenNeural", "zh-TW"),
        Some("cantonese") => ("zh-HK-HiuMaanNeural", "zh-HK"),
        Some("japanese") => ("ja-JP-NanamiNeural", "ja-JP"),
        Some("korean") => ("ko-KR-SunHiNeural", "ko-KR"),
        Some("thai") => ("th-TH-PremwadeeNeural", "th-TH"),
        Some("french") => ("fr-FR-DeniseNeural", "fr-FR"),
        Some("german") => ("de-DE-KatjaNeural", "de-DE"),
        Some("spanish") => ("es-ES-ElviraNeural", "es-ES"),
        Some("portuguese") => ("pt-BR-FranciscaNeural", "pt-BR"),
        Some("italian") => ("it-IT-ElsaNeural", "it-IT"),
        Some("russian") => ("ru-RU-SvetlanaNeural", "ru-RU"),
        Some("arabic") => ("ar-SA-ZariyahNeural", "ar-SA"),
        Some("hindi") => ("hi-IN-SwaraNeural", "hi-IN"),
        Some("indonesian") => ("id-ID-GadisNeural", "id-ID"),
        _ => ("en-US-AvaMultilingualNeural", "en-US"),
    }
}

/// The offline Kokoro v1.1-zh voice speaks English and Mandarin only. Returns the
/// speaker id to use, or `None` when the text needs a language it cannot speak.
pub fn local(language: Option<&str>, text: &str) -> Option<i32> {
    match language.map(normalise).as_deref() {
        Some("english") => Some(LOCAL_ENGLISH_SPEAKER),
        Some("simplified chinese" | "traditional chinese" | "chinese") => {
            Some(LOCAL_CHINESE_SPEAKER)
        }
        Some(_) => None,
        // No language given (the original side of a translation): judge by the text.
        None => {
            let mut han = false;
            for c in text.chars() {
                if is_han(c) {
                    han = true;
                } else if c.is_alphabetic() && !c.is_ascii() {
                    // Accented Latin, Cyrillic, Thai, kana… — not English or Chinese.
                    return None;
                }
            }
            Some(if han {
                LOCAL_CHINESE_SPEAKER
            } else {
                LOCAL_ENGLISH_SPEAKER
            })
        }
    }
}

/// `af_maple`, the first of Kokoro v1.1-zh's three English speakers.
pub const LOCAL_ENGLISH_SPEAKER: i32 = 0;
/// `zf_001`, the first Mandarin speaker. It reads embedded English words too.
pub const LOCAL_CHINESE_SPEAKER: i32 = 3;

/// Kokoro v1.1-zh's speakers in speaker-id order, as published for sherpa-onnx's
/// kokoro-multi-lang-v1_1: three English, then Mandarin female (zf) and male (zm).
const LOCAL_SPEAKERS: [&str; 103] = [
    "af_maple", "af_sol", "bf_vale", "zf_001", "zf_002", "zf_003", "zf_004", "zf_005",
    "zf_006", "zf_007", "zf_008", "zf_017", "zf_018", "zf_019", "zf_021", "zf_022", "zf_023",
    "zf_024", "zf_026", "zf_027", "zf_028", "zf_032", "zf_036", "zf_038", "zf_039", "zf_040",
    "zf_042", "zf_043", "zf_044", "zf_046", "zf_047", "zf_048", "zf_049", "zf_051", "zf_059",
    "zf_060", "zf_067", "zf_070", "zf_071", "zf_072", "zf_073", "zf_074", "zf_075", "zf_076",
    "zf_077", "zf_078", "zf_079", "zf_083", "zf_084", "zf_085", "zf_086", "zf_087", "zf_088",
    "zf_090", "zf_092", "zf_093", "zf_094", "zf_099", "zm_009", "zm_010", "zm_011", "zm_012",
    "zm_013", "zm_014", "zm_015", "zm_016", "zm_020", "zm_025", "zm_029", "zm_030", "zm_031",
    "zm_033", "zm_034", "zm_035", "zm_037", "zm_041", "zm_045", "zm_050", "zm_052", "zm_053",
    "zm_054", "zm_055", "zm_056", "zm_057", "zm_058", "zm_061", "zm_062", "zm_063", "zm_064",
    "zm_065", "zm_066", "zm_068", "zm_069", "zm_080", "zm_081", "zm_082", "zm_089", "zm_091",
    "zm_095", "zm_096", "zm_097", "zm_098", "zm_100",
];

/// The offline speakers that can read `language` (one of the app's language names).
pub fn local_voices(language: &str) -> Vec<(String, String)> {
    let Some(default) = local(Some(language), "") else {
        return Vec::new();
    };
    let chinese = default == LOCAL_CHINESE_SPEAKER;
    LOCAL_SPEAKERS
        .iter()
        .enumerate()
        .filter(|(id, _)| is_chinese_speaker(*id as i32) == chinese)
        .map(|(id, name)| (id.to_string(), local_label(name)))
        .collect()
}

/// A speaker picked in Settings, if it can read the text it is being asked to read.
/// A Chinese speaker on an English sentence (or the reverse) falls back to the default.
pub fn local_pick(chosen: Option<&str>, language: Option<&str>, text: &str) -> Option<i32> {
    let default = local(language, text)?;
    let picked = chosen
        .and_then(|id| id.parse::<i32>().ok())
        .filter(|&id| (0..LOCAL_SPEAKERS.len() as i32).contains(&id))
        .filter(|&id| is_chinese_speaker(id) == is_chinese_speaker(default));
    Some(picked.unwrap_or(default))
}

pub fn is_chinese_speaker(id: i32) -> bool {
    id >= LOCAL_CHINESE_SPEAKER
}

fn local_label(name: &str) -> String {
    let kind = match &name[..2] {
        "af" => "American English, female",
        "bf" => "British English, female",
        "zf" => "Mandarin, female",
        _ => "Mandarin, male",
    };
    format!("{name} — {kind}")
}

/// The prefix MiniMax gives every system voice of a language, for filtering
/// `/v1/get_voice` down to the voices that suit it.
pub fn minimax_prefix(language: &str) -> Option<&'static str> {
    Some(match normalise(language).as_str() {
        "english" => "English_",
        "vietnamese" => "Vietnamese_",
        "simplified chinese" | "traditional chinese" | "chinese" => "Chinese (Mandarin)_",
        "cantonese" => "Cantonese_",
        "japanese" => "Japanese_",
        "korean" => "Korean_",
        "thai" => "Thai_",
        "french" => "French_",
        "german" => "German_",
        "spanish" => "Spanish_",
        "portuguese" => "Portuguese_",
        "italian" => "Italian_",
        "russian" => "Russian_",
        "arabic" => "Arabic_",
        "hindi" => "hindi_",
        "indonesian" => "Indonesian_",
        _ => return None,
    })
}

/// The locale of an Azure voice name: "vi-VN-HoaiMyNeural" -> "vi-VN".
pub fn azure_locale(voice: &str) -> String {
    voice.splitn(3, '-').take(2).collect::<Vec<_>>().join("-")
}

/// Which Azure locales suit a language, for filtering the region's voice list.
pub fn azure_locales(language: &str) -> &'static [&'static str] {
    match normalise(language).as_str() {
        "english" => &["en-US", "en-GB", "en-AU", "en-CA", "en-IN"],
        "vietnamese" => &["vi-VN"],
        "simplified chinese" | "chinese" => &["zh-CN"],
        "traditional chinese" => &["zh-TW", "zh-HK"],
        "cantonese" => &["zh-HK", "yue-CN"],
        "japanese" => &["ja-JP"],
        "korean" => &["ko-KR"],
        "thai" => &["th-TH"],
        "french" => &["fr-FR", "fr-CA"],
        "german" => &["de-DE"],
        "spanish" => &["es-ES", "es-MX"],
        "portuguese" => &["pt-BR", "pt-PT"],
        "italian" => &["it-IT"],
        "russian" => &["ru-RU"],
        "arabic" => &["ar-SA", "ar-EG"],
        "hindi" => &["hi-IN"],
        "indonesian" => &["id-ID"],
        _ => &[],
    }
}

fn normalise(language: &str) -> String {
    language.trim().to_lowercase()
}

fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_offline_voice_only_takes_english_and_chinese() {
        assert_eq!(local(Some("English"), "hi"), Some(LOCAL_ENGLISH_SPEAKER));
        assert_eq!(local(Some("Simplified Chinese"), "你好"), Some(LOCAL_CHINESE_SPEAKER));
        assert_eq!(local(Some("Vietnamese"), "xin chào"), None);
    }

    #[test]
    fn an_unlabelled_text_is_judged_by_its_script() {
        assert_eq!(local(None, "Where is the station?"), Some(LOCAL_ENGLISH_SPEAKER));
        assert_eq!(local(None, "我们用 GitHub 来管理代码"), Some(LOCAL_CHINESE_SPEAKER));
        assert_eq!(local(None, "Xin chào các bạn"), None);
        assert_eq!(local(None, "Привет"), None);
    }

    #[test]
    fn a_picked_offline_speaker_must_suit_the_text() {
        // A Mandarin speaker picked for Chinese is used for Chinese…
        assert_eq!(local_pick(Some("60"), Some("Simplified Chinese"), "你好"), Some(60));
        // …but not for an English sentence, which gets the English default.
        assert_eq!(local_pick(Some("60"), Some("English"), "hi"), Some(LOCAL_ENGLISH_SPEAKER));
        assert_eq!(local_pick(Some("999"), Some("English"), "hi"), Some(LOCAL_ENGLISH_SPEAKER));
        assert_eq!(local_voices("English").len(), 3);
        assert_eq!(local_voices("Simplified Chinese").len(), 100);
        assert!(local_voices("Vietnamese").is_empty());
    }

    #[test]
    fn azure_locales_come_from_the_voice_name() {
        assert_eq!(azure_locale("vi-VN-HoaiMyNeural"), "vi-VN");
        assert_eq!(azure_locale("en-US-Ava:DragonHDLatestNeural"), "en-US");
    }

    #[test]
    fn unknown_languages_fall_back_to_a_multilingual_voice() {
        assert_eq!(minimax(Some("Klingon")).1, "auto");
        assert_eq!(azure(None).0, "en-US-AvaMultilingualNeural");
    }
}
