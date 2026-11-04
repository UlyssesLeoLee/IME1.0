//! The voice input: the microphone is recorded, the speech is recognised, the text is written where the user types.
//!
//! The recording happens in this process (a small buffer of PCM, nothing else), the recognition happens in the AI
//! worker - the only process that ever sees the API key - and the text is written with the keyboard input of the
//! system, so that it lands in whatever program the user is in.
//!
//! The API key is never written to a file here: it is handed to the worker once (`cred.set`) and lives in the
//! Windows Credential Manager from then on. This process only knows the name it goes by.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pime_core::json::{self, Json, Limits};
use windows::core::PSTR;
use windows::Win32::Media::Audio::{
    CALLBACK_NULL, HWAVEIN, WAVE_FORMAT_PCM, WAVE_MAPPER, WAVEFORMATEX, WAVEHDR, WHDR_DONE, waveInAddBuffer,
    waveInClose, waveInOpen, waveInReset, waveInStart, waveInStop,
};

/// The sample rate the speech services expect (16 kHz, mono).
pub const SAMPLE_RATE: u32 = 16000;

/// How much audio one buffer of the recording holds (a tenth of a second).
const BUFFER_BYTES: usize = 3200;

/// How many buffers the recording has.
const BUFFER_COUNT: usize = 8;

/// The longest recording that is accepted (five minutes).
const MAX_RECORDING_BYTES: usize = SAMPLE_RATE as usize * 2 * 300;

/// Why the voice input could not do what it was asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// There is no microphone, or Windows refuses to open it.
    NoMicrophone,
    /// The AI worker is not there (it is installed next to this program).
    NoWorker,
    /// The speech service is not configured yet (no address, no key).
    NotConfigured,
    /// The service answered with an error.
    Refused,
    /// The service answered, but there was no text in it.
    NoText,
}

impl Reason {
    /// The text the user is shown.
    pub fn message(self) -> &'static str {
        match self {
            Reason::NoMicrophone => "没有可用的麦克风",
            Reason::NoWorker => "找不到语音服务组件（pime-ai-worker.exe）",
            Reason::NotConfigured => "语音输入还没有设置（先运行 pime-tray --set-voice）",
            Reason::Refused => "语音服务拒绝了这次请求",
            Reason::NoText => "没有听清",
        }
    }
}

/// A recording in progress.
pub struct Voice {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<Result<Vec<u8>, Reason>>>,
}

impl Voice {
    /// Start recording.
    pub fn start() -> Result<Voice, Reason> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("pime-voice".to_string())
            .spawn(move || record(flag))
            .map_err(|_| Reason::NoMicrophone)?;
        Ok(Voice {
            stop,
            handle: Some(handle),
        })
    }

    /// The recording in progress, as the audio of the microphone (a WAV file).
    pub fn into_audio(mut self) -> Result<Vec<u8>, Reason> {
        self.stop.store(true, Ordering::Relaxed);
        match self.handle.take() {
            Some(handle) => handle.join().unwrap_or(Err(Reason::NoMicrophone)),
            None => Err(Reason::NoMicrophone),
        }
    }
}

/// Stop the recording, recognise the speech and write the text where the user is typing.
pub fn finish(voice: Voice) {
    let audio = match voice.into_audio() {
        Ok(audio) => audio,
        Err(reason) => {
            report(reason);
            return;
        }
    };
    // The network must not block the tray: the text is written when the answer arrives.
    std::thread::Builder::new()
        .name("pime-voice-asr".to_string())
        .spawn(move || {
            match transcribe(&audio) {
                Ok(text) => write_text(&text),
                Err(reason) => report(reason),
            }
        })
        .ok();
}

/// Tell the user that something did not work (a balloon of the icon).
pub fn report(reason: Reason) {
    // The message is written to the standard error: the tray has no window of its own to show a balloon in, and the
    // user runs the program from a console when something goes wrong.
    eprintln!("IME1.0: {}", reason.message());
}

/// The recording itself: the microphone, in a thread of its own.
fn record(stop: Arc<AtomicBool>) -> Result<Vec<u8>, Reason> {
    let format = WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM as u16,
        nChannels: 1,
        nSamplesPerSec: SAMPLE_RATE,
        nAvgBytesPerSec: SAMPLE_RATE * 2,
        nBlockAlign: 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    let mut device = HWAVEIN::default();
    // SAFETY: the format is a live local and the device is a live out pointer.
    let opened = unsafe {
        waveInOpen(
            Some(&mut device),
            WAVE_MAPPER,
            &format,
            0,
            0,
            CALLBACK_NULL,
        )
    };
    if opened != 0 {
        return Err(Reason::NoMicrophone);
    }
    let mut buffers: Vec<Vec<u8>> = (0..BUFFER_COUNT)
        .map(|_| vec![0u8; BUFFER_BYTES])
        .collect();
    let mut headers: Vec<WAVEHDR> = (0..BUFFER_COUNT)
        .map(|_| WAVEHDR {
            lpData: PSTR::null(),
            dwBufferLength: 0,
            dwBytesRecorded: 0,
            dwUser: 0,
            dwFlags: 0,
            dwLoops: 0,
            lpNext: std::ptr::null_mut(),
            reserved: 0,
        })
        .collect();
    for (header, buffer) in headers.iter_mut().zip(buffers.iter_mut()) {
        header.lpData = PSTR(buffer.as_mut_ptr());
        header.dwBufferLength = BUFFER_BYTES as u32;
        // SAFETY: the header is live and points at the buffer, which outlives it.
        unsafe { waveInAddBuffer(device, header, std::mem::size_of::<WAVEHDR>() as u32) };
    }
    // SAFETY: the device is live and the buffers are queued.
    unsafe { waveInStart(device) };

    let mut recorded: Vec<u8> = Vec::new();
    while !stop.load(Ordering::Relaxed) && recorded.len() < MAX_RECORDING_BYTES {
        std::thread::sleep(Duration::from_millis(20));
        for (header, buffer) in headers.iter_mut().zip(buffers.iter_mut()) {
            let flags = header.dwFlags;
            let done = header.dwBytesRecorded;
            if flags & WHDR_DONE == 0 {
                continue;
            }
            if done > 0 {
                let end = (done as usize).min(buffer.len());
                recorded.extend_from_slice(&buffer[..end]);
            }
            header.dwFlags = flags & !WHDR_DONE;
            header.dwBytesRecorded = 0;
            // SAFETY: the header is live and points at the buffer, which outlives it.
            unsafe { waveInAddBuffer(device, header, std::mem::size_of::<WAVEHDR>() as u32) };
        }
    }
    // SAFETY: the device is live and the recording is over.
    unsafe {
        waveInStop(device);
        waveInReset(device);
        waveInClose(device);
    }
    Ok(wav(&recorded))
}

/// The audio of the microphone as a WAV file (the format every speech service reads).
pub fn wav(pcm: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + pcm.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // one channel
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // the block
    out.extend_from_slice(&16u16.to_le_bytes()); // the bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// The body of a request to a speech service: the recording and the two fields that go with it.
///
/// The form is the one every OpenAI-compatible service reads (`multipart/form-data`).
pub fn multipart(boundary: &str, model: &str, language: &str, audio: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(audio.len() + 512);
    let field = |name: &str, value: &str, body: &mut Vec<u8>| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    };
    field("model", model, &mut body);
    field("language", language, &mut body);
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        b"Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\nContent-Type: audio/wav\r\n\r\n",
    );
    body.extend_from_slice(audio);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

/// The speech service the user chose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The address of the service (`https://api.openai.com/v1/audio/transcriptions`).
    pub endpoint: String,
    /// The model of the service (`whisper-1`).
    pub model: String,
    /// The language of the speech (`zh`, `en`, `ja`, ...); empty means "let the service decide".
    pub language: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            endpoint: "https://api.openai.com/v1/audio/transcriptions".to_string(),
            model: "whisper-1".to_string(),
            language: String::new(),
        }
    }
}

/// The registry key where the settings of the voice input live.
const SETTINGS_KEY: &str = "Software\\IME1.0";
/// The value that holds them.
const SETTINGS_VALUE: &str = "Voice";

impl Settings {
    /// The text of the settings, as they are written to the registry.
    pub fn to_record(&self) -> String {
        format!("{}|{}|{}", self.endpoint, self.model, self.language)
    }

    /// The settings of a record that was written before. The defaults are used for the parts that are missing.
    pub fn from_record(record: &str) -> Settings {
        let mut parts = record.split('|');
        let endpoint = parts.next().unwrap_or("").trim().to_string();
        let model = parts.next().unwrap_or("").trim().to_string();
        let language = parts.next().unwrap_or("").trim().to_string();
        let defaults = Settings::default();
        Settings {
            endpoint: if endpoint.is_empty() {
                defaults.endpoint
            } else {
                endpoint
            },
            model: if model.is_empty() {
                defaults.model
            } else {
                model
            },
            language,
        }
    }
}

/// The settings of the voice input, as the user has them.
pub fn load_settings() -> Settings {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW, RegOpenKeyExW, KEY_QUERY_VALUE,
    };
    let key: Vec<u16> = SETTINGS_KEY.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = SETTINGS_VALUE.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handle = windows::Win32::System::Registry::HKEY::default();
    // SAFETY: the key name is NUL-terminated and the handle is a live out pointer.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            0,
            KEY_QUERY_VALUE,
            &mut handle,
        )
    };
    if opened != ERROR_SUCCESS {
        return Settings::default();
    }
    let mut size = 0u32;
    // SAFETY: the key is live and the value name is NUL-terminated; nothing is read yet.
    let read = unsafe {
        RegGetValueW(
            handle,
            PCWSTR::null(),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if read != ERROR_SUCCESS || size == 0 {
        // SAFETY: closes the handle opened above, once.
        let _ = unsafe { RegCloseKey(handle) };
        return Settings::default();
    }
    let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
    // SAFETY: the buffer is writable for `size` bytes and the key is live.
    let read = unsafe {
        RegGetValueW(
            handle,
            PCWSTR::null(),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast::<core::ffi::c_void>()),
            Some(&mut size),
        )
    };
    // SAFETY: closes the handle opened above, once.
    let _ = unsafe { RegCloseKey(handle) };
    if read != ERROR_SUCCESS {
        return Settings::default();
    }
    let text = String::from_utf16_lossy(&buffer[..buffer.len().saturating_sub(1)]);
    Settings::from_record(text.trim_end_matches('\0').trim())
}

/// Keep the settings of the voice input.
pub fn save_settings(settings: &Settings) {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
        RegSetValueExW,
    };
    let key: Vec<u16> = SETTINGS_KEY.encode_utf16().chain(std::iter::once(0)).collect();
    let value: Vec<u16> = SETTINGS_VALUE.encode_utf16().chain(std::iter::once(0)).collect();
    let mut handle = windows::Win32::System::Registry::HKEY::default();
    // SAFETY: the key name is NUL-terminated and the handle is a live out pointer.
    let created = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut handle,
            None,
        )
    };
    if created != ERROR_SUCCESS {
        return;
    }
    let data: Vec<u8> = settings
        .to_record()
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|u| u.to_le_bytes())
        .collect();
    // SAFETY: the key is live, the name is NUL-terminated and the data is the text with its terminator.
    let _ = unsafe {
        RegSetValueExW(
            handle,
            PCWSTR(value.as_ptr()),
            0,
            REG_SZ,
            Some(data.as_slice()),
        )
    };
    // SAFETY: closes the handle opened above, once.
    let _ = unsafe { RegCloseKey(handle) };
}

/// The name the API key goes by in the Credential Manager.
pub const CREDENTIAL_REF: &str = "voice-asr";

/// Keep the API key in the Windows Credential Manager.
///
/// The key is handed to the AI worker, which puts it there and forgets it: from then on this program only knows the
/// name the key goes by, and the key itself lives in the store of Windows.
pub fn store_key(key: &str, endpoint: &str) -> Result<(), Reason> {
    let key = key.trim();
    if key.is_empty() {
        return Err(Reason::NotConfigured);
    }
    let host = host_of(endpoint).ok_or(Reason::NotConfigured)?;
    let request = Json::obj(vec![
        ("op", Json::str("cred.set")),
        ("id", Json::int(1)),
        ("credentialRef", Json::str(CREDENTIAL_REF)),
        ("host", Json::str(&host)),
        ("secret", Json::str(key)),
    ]);
    let reply = talk(&[request])?;
    if reply.get("ok").and_then(Json::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(Reason::Refused)
    }
}

/// Take the API key out of the Windows Credential Manager.
pub fn forget_key() -> Result<(), Reason> {
    let request = Json::obj(vec![
        ("op", Json::str("cred.delete")),
        ("id", Json::int(1)),
        ("credentialRef", Json::str(CREDENTIAL_REF)),
    ]);
    let reply = talk(&[request])?;
    if reply.get("ok").and_then(Json::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(Reason::Refused)
    }
}

/// Recognise the speech in the recording.
pub fn transcribe(audio: &[u8]) -> Result<String, Reason> {
    let settings = load_settings();
    if host_of(&settings.endpoint).is_none() {
        return Err(Reason::NotConfigured);
    }
    let boundary = "ime1boundary";
    let body = multipart(boundary, &settings.model, &settings.language, audio);
    let request = Json::obj(vec![
        ("op", Json::str("http.send")),
        ("id", Json::int(1)),
        ("method", Json::str("POST")),
        ("url", Json::str(&settings.endpoint)),
        (
            "headers",
            Json::obj(vec![(
                "Content-Type",
                Json::str(&format!("multipart/form-data; boundary={boundary}")),
            )]),
        ),
        ("bodyBase64", Json::str(&pime_core::base64::encode(&body))),
        ("credentialRef", Json::str(CREDENTIAL_REF)),
        ("authHeader", Json::str("Authorization: Bearer {key}")),
    ]);
    let reply = talk(&[request])?;
    if reply.get("ok").and_then(Json::as_bool) != Some(true) {
        return Err(Reason::Refused);
    }
    let answer = reply.get("body").and_then(Json::as_str).unwrap_or("");
    let parsed = json::parse(answer, Limits::DEFAULT).map_err(|_| Reason::Refused)?;
    let text = parsed
        .get("text")
        .and_then(Json::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        return Err(Reason::NoText);
    }
    Ok(text)
}

/// The host of an address (`https://api.openai.com/v1/...` -> `api.openai.com`).
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.split('@').next_back()?;
    if host.is_empty() { None } else { Some(host.to_string()) }
}

/// Send the requests to the AI worker and read the answers.
fn talk(requests: &[Json]) -> Result<Json, Reason> {
    let worker = worker_exe().ok_or(Reason::NoWorker)?;
    let mut child = Command::new(worker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Reason::NoWorker)?;
    let mut input = child.stdin.take().ok_or(Reason::NoWorker)?;
    let mut output = std::io::BufReader::new(child.stdout.take().ok_or(Reason::NoWorker)?);
    for request in requests {
        let line = request.to_json_string();
        if input.write_all(line.as_bytes()).is_err() || input.write_all(b"\n").is_err() {
            return Err(Reason::NoWorker);
        }
        if input.flush().is_err() {
            return Err(Reason::NoWorker);
        }
        let mut answer = String::new();
        if output.read_line(&mut answer).is_err() || answer.trim().is_empty() {
            return Err(Reason::NoWorker);
        }
        let parsed = json::parse(answer.trim(), Limits::DEFAULT).map_err(|_| Reason::NoWorker)?;
        if request.get("id").is_some() {
            return Ok(parsed);
        }
    }
    Err(Reason::NoWorker)
}

/// The AI worker, which sits next to this program.
fn worker_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let folder = exe.parent()?;
    let worker = folder.join("pime-ai-worker.exe");
    worker.is_file().then_some(worker)
}

/// Write the text into the program the user is in.
pub fn write_text(text: &str) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, SendInput,
        VIRTUAL_KEY,
    };
    let mut inputs: Vec<INPUT> = Vec::with_capacity(text.chars().count() * 2);
    for unit in text.encode_utf16() {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYBD_EVENT_FLAGS(KEYEVENTF_UNICODE.0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    for unit in text.encode_utf16() {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0),
                    wScan: unit,
                    dwFlags: KEYBD_EVENT_FLAGS((KEYEVENTF_UNICODE | KEYEVENTF_KEYUP).0),
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    let size = std::mem::size_of::<INPUT>() as i32;
    // SAFETY: the array is live and holds exactly the events that are sent.
    unsafe {
        SendInput(&inputs, size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_is_a_wave_file() {
        let audio = wav(&[1, 2, 3, 4]);
        assert_eq!(&audio[0..4], b"RIFF");
        assert_eq!(&audio[8..12], b"WAVE");
        assert_eq!(&audio[12..16], b"fmt ");
        assert_eq!(&audio[36..40], b"data");
        let size = u32::from_le_bytes(audio[40..44].try_into().unwrap());
        assert_eq!(size as usize, audio.len() - 44);
        assert_eq!(&audio[44..], &[1, 2, 3, 4]);
    }

    #[test]
    fn the_form_carries_the_model_the_language_and_the_audio() {
        let body = multipart("BOUND", "whisper-1", "zh", b"AUDIO");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("--BOUND\r\n"), "{text}");
        assert!(text.contains("name=\"model\"\r\n\r\nwhisper-1\r\n"), "{text}");
        assert!(text.contains("name=\"language\"\r\n\r\nzh\r\n"), "{text}");
        assert!(
            text.contains("name=\"file\"; filename=\"audio.wav\""),
            "{text}"
        );
        assert!(text.contains("Content-Type: audio/wav"), "{text}");
        assert!(text.ends_with("--BOUND--\r\n"), "{text}");
        assert!(body.windows(5).any(|w| w == b"AUDIO"));
    }

    #[test]
    fn the_host_of_an_address_is_read() {
        assert_eq!(
            host_of("https://api.openai.com/v1/audio/transcriptions"),
            Some("api.openai.com".to_string())
        );
        assert_eq!(host_of("https://host:8443/x"), Some("host:8443".to_string()));
        assert_eq!(host_of("not a url"), None);
        assert_eq!(host_of("https://"), None);
    }
}
