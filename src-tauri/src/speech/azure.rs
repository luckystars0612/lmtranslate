//! Azure AI Speech, through its REST endpoint: one SSML request per clip, MP3 back.

use serde::{Deserialize, Serialize};

use super::{voices, Clip, SpeechError};
use crate::config::Settings;

const OUTPUT_FORMAT: &str = "audio-24khz-96kbitrate-mono-mp3";

/// The regions offered in Settings: (id, label). The id is what Azure calls the region
/// and what goes into the endpoint's host name.
pub const REGIONS: &[(&str, &str)] = &[
    ("southeastasia", "Southeast Asia (Singapore)"),
    ("eastasia", "East Asia (Hong Kong)"),
    ("japaneast", "Japan East (Tokyo)"),
    ("japanwest", "Japan West (Osaka)"),
    ("koreacentral", "Korea Central (Seoul)"),
    ("centralindia", "Central India (Pune)"),
    ("australiaeast", "Australia East (Sydney)"),
    ("eastus", "East US (Virginia)"),
    ("eastus2", "East US 2 (Virginia)"),
    ("centralus", "Central US (Iowa)"),
    ("northcentralus", "North Central US (Illinois)"),
    ("southcentralus", "South Central US (Texas)"),
    ("westcentralus", "West Central US (Wyoming)"),
    ("westus", "West US (California)"),
    ("westus2", "West US 2 (Washington)"),
    ("westus3", "West US 3 (Arizona)"),
    ("canadacentral", "Canada Central (Toronto)"),
    ("brazilsouth", "Brazil South (São Paulo)"),
    ("northeurope", "North Europe (Ireland)"),
    ("westeurope", "West Europe (Netherlands)"),
    ("uksouth", "UK South (London)"),
    ("francecentral", "France Central (Paris)"),
    ("germanywestcentral", "Germany West Central (Frankfurt)"),
    ("italynorth", "Italy North (Milan)"),
    ("swedencentral", "Sweden Central (Gävle)"),
    ("switzerlandnorth", "Switzerland North (Zürich)"),
    ("switzerlandwest", "Switzerland West (Geneva)"),
    ("norwayeast", "Norway East (Oslo)"),
    ("uaenorth", "UAE North (Dubai)"),
    ("qatarcentral", "Qatar Central (Doha)"),
    ("southafricanorth", "South Africa North (Johannesburg)"),
    ("chinaeast2", "China East 2 (Azure China)"),
    ("chinanorth2", "China North 2 (Azure China)"),
    ("chinanorth3", "China North 3 (Azure China)"),
    ("usgovarizona", "US Gov Arizona (Azure Government)"),
    ("usgovvirginia", "US Gov Virginia (Azure Government)"),
];

/// The TTS host for a region. Azure China and Azure Government are separate clouds
/// with their own domains; everything else is on microsoft.com.
pub fn host(region: &str) -> String {
    let domain = if region.starts_with("china") {
        "azure.cn"
    } else if region.starts_with("usgov") {
        "azure.us"
    } else {
        "microsoft.com"
    };
    format!("https://{region}.tts.speech.{domain}")
}

/// One entry of the region's `/cognitiveservices/voices/list`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AzureVoice {
    pub short_name: String,
    #[serde(default)]
    pub local_name: String,
    #[serde(default)]
    pub locale: String,
    #[serde(default)]
    pub gender: String,
    #[serde(default)]
    pub secondary_locale_list: Vec<String>,
}

pub struct AzureSpeech {
    http: reqwest::Client,
    key: String,
    region: String,
}

impl AzureSpeech {
    pub fn from_settings(settings: &Settings, http: reqwest::Client) -> Result<Self, SpeechError> {
        let key = settings.azure_speech_key.trim();
        let region = settings.azure_speech_region.trim();
        if key.is_empty() || region.is_empty() {
            return Err(SpeechError::NotConfigured(
                "no Azure Speech key and region".into(),
            ));
        }
        Ok(Self {
            http,
            key: key.to_string(),
            region: region.to_lowercase(),
        })
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub async fn synthesize(&self, text: &str, voice: &str) -> Result<Clip, SpeechError> {
        let locale = voices::azure_locale(voice);
        let ssml = format!(
            "<speak version='1.0' xml:lang='{locale}' xmlns='http://www.w3.org/2001/10/synthesis'>\
             <voice name='{}'>{}</voice></speak>",
            escape(voice),
            escape(text)
        );

        let response = self
            .http
            .post(format!("{}/cognitiveservices/v1", host(&self.region)))
            .header("Ocp-Apim-Subscription-Key", &self.key)
            .header("Content-Type", "application/ssml+xml")
            .header("X-Microsoft-OutputFormat", OUTPUT_FORMAT)
            .body(ssml)
            .send()
            .await
            .map_err(|e| self.unreachable(e))?;

        let status = response.status().as_u16();
        if status != 200 {
            let body = response.text().await.unwrap_or_default();
            return Err(self.refused(status, &body));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| SpeechError::Network(format!("Azure stopped mid-answer: {e}")))?;
        if bytes.is_empty() {
            return Err(SpeechError::Failed(format!(
                "Azure returned no audio — is \"{voice}\" available in {}?",
                self.region
            )));
        }
        Ok(Clip {
            mime: "audio/mpeg",
            bytes: bytes.to_vec(),
        })
    }

    /// Every voice the region offers. Voices differ between regions, so this is what
    /// Settings lists rather than a fixed table.
    pub async fn list_voices(&self) -> Result<Vec<AzureVoice>, SpeechError> {
        let response = self
            .http
            .get(format!("{}/cognitiveservices/voices/list", host(&self.region)))
            .header("Ocp-Apim-Subscription-Key", &self.key)
            .send()
            .await
            .map_err(|e| self.unreachable(e))?;
        let status = response.status().as_u16();
        if status != 200 {
            let body = response.text().await.unwrap_or_default();
            return Err(self.refused(status, &body));
        }
        response
            .json()
            .await
            .map_err(|e| SpeechError::Failed(format!("Azure sent an unreadable voice list: {e}")))
    }

    fn unreachable(&self, e: reqwest::Error) -> SpeechError {
        SpeechError::Network(format!(
            "could not reach Azure Speech in region \"{}\": {e}",
            self.region
        ))
    }

    fn refused(&self, status: u16, body: &str) -> SpeechError {
        match status {
            401 | 403 => SpeechError::Rejected(format!(
                "Azure rejected the Speech key. Check that the key belongs to a resource in \
                 {}.",
                self.region
            )),
            429 => SpeechError::Rejected(
                "Azure is rate limiting this key, or its free quota is used up.".into(),
            ),
            _ => SpeechError::Failed(format!(
                "Azure Speech answered HTTP {status}: {}",
                body.chars().take(200).collect::<String>()
            )),
        }
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_cannot_break_out_of_the_ssml() {
        assert_eq!(
            escape("a < b & 'c' </voice>"),
            "a &lt; b &amp; &apos;c&apos; &lt;/voice&gt;"
        );
    }

    #[test]
    fn sovereign_clouds_get_their_own_domain() {
        assert_eq!(host("southeastasia"), "https://southeastasia.tts.speech.microsoft.com");
        assert_eq!(host("chinaeast2"), "https://chinaeast2.tts.speech.azure.cn");
        assert_eq!(host("usgovvirginia"), "https://usgovvirginia.tts.speech.azure.us");
    }
}
