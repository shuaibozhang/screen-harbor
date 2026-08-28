use windows::{
    core::Error as WindowsError,
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::{
            eCapture, eMultimedia, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
        },
        System::Com::StructuredStorage::PropVariantToString,
        System::Com::{CoCreateInstance, CLSCTX_ALL, STGM_READ},
    },
};

use crate::{com::ComApartment, PlatformError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEndpointInfo {
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefaultAudioEndpoints {
    pub microphone: Option<AudioEndpointInfo>,
    pub system_output: Option<AudioEndpointInfo>,
}

/// Discovers the current Windows multimedia-role endpoints. Device changes are
/// intentionally resolved on demand so the main window reflects the active
/// defaults without restarting FrameFlow.
pub fn default_audio_endpoints() -> Result<DefaultAudioEndpoints, PlatformError> {
    let com = ComApartment::enter_multithreaded()?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let endpoints = DefaultAudioEndpoints {
        microphone: default_endpoint(&enumerator, eCapture),
        system_output: default_endpoint(&enumerator, eRender),
    };
    drop(com);
    Ok(endpoints)
}

fn default_endpoint(
    enumerator: &IMMDeviceEnumerator,
    flow: windows::Win32::Media::Audio::EDataFlow,
) -> Option<AudioEndpointInfo> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eMultimedia) }.ok()?;
    endpoint_name(&device)
        .ok()
        .filter(|name| !name.is_empty())
        .map(|name| AudioEndpointInfo { name })
}

fn endpoint_name(device: &IMMDevice) -> Result<String, WindowsError> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }?;
    let value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }?;
    let mut buffer = [0_u16; 512];
    unsafe { PropVariantToString(&value, &mut buffer) }?;
    Ok(utf16_buffer_to_string(&buffer))
}

fn utf16_buffer_to_string(buffer: &[u16]) -> String {
    let length = buffer
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..length])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_null_terminated_utf16_names() {
        assert_eq!(utf16_buffer_to_string(&[0x9ed8, 0x8ba4, 0, 0x58]), "默认");
    }
}
