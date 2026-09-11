use windows::{
    core::Error as WindowsError,
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Media::Audio::{
            eCapture, eMultimedia, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
            DEVICE_STATE_ACTIVE,
        },
        System::Com::StructuredStorage::PropVariantToString,
        System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ},
    },
};

use crate::{com::ComApartment, PlatformError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEndpointInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioEndpoints {
    pub microphones: Vec<AudioEndpointInfo>,
    pub system_outputs: Vec<AudioEndpointInfo>,
    pub default_microphone_id: Option<String>,
    pub default_system_output_id: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefaultAudioEndpoints {
    pub microphone: Option<AudioEndpointInfo>,
    pub system_output: Option<AudioEndpointInfo>,
}

/// Discovers the current Windows multimedia-role endpoints. Device changes are
/// intentionally resolved on demand so the main window reflects the active
/// defaults without restarting ScreenHarbor.
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

/// Enumerates all active capture and render endpoints and identifies the
/// current multimedia-role defaults. Endpoint IDs are stable Windows device
/// identifiers and can be passed back when starting a capture session.
pub fn audio_endpoints() -> Result<AudioEndpoints, PlatformError> {
    let com = ComApartment::enter_multithreaded()?;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let default_microphone_id = default_endpoint_id(&enumerator, eCapture);
    let default_system_output_id = default_endpoint_id(&enumerator, eRender);
    let endpoints = AudioEndpoints {
        microphones: enumerate_endpoints(&enumerator, eCapture),
        system_outputs: enumerate_endpoints(&enumerator, eRender),
        default_microphone_id,
        default_system_output_id,
    };
    drop(com);
    Ok(endpoints)
}

fn default_endpoint(
    enumerator: &IMMDeviceEnumerator,
    flow: windows::Win32::Media::Audio::EDataFlow,
) -> Option<AudioEndpointInfo> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eMultimedia) }.ok()?;
    endpoint_info(&device)
}

fn default_endpoint_id(
    enumerator: &IMMDeviceEnumerator,
    flow: windows::Win32::Media::Audio::EDataFlow,
) -> Option<String> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(flow, eMultimedia) }.ok()?;
    endpoint_id(&device).ok()
}

fn enumerate_endpoints(
    enumerator: &IMMDeviceEnumerator,
    flow: windows::Win32::Media::Audio::EDataFlow,
) -> Vec<AudioEndpointInfo> {
    let Ok(collection) = (unsafe { enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) })
    else {
        return Vec::new();
    };
    let Ok(count) = (unsafe { collection.GetCount() }) else {
        return Vec::new();
    };
    (0..count)
        .filter_map(|index| unsafe { collection.Item(index) }.ok())
        .filter_map(|device| endpoint_info(&device))
        .collect()
}

fn endpoint_info(device: &IMMDevice) -> Option<AudioEndpointInfo> {
    let id = endpoint_id(device).ok()?;
    let name = endpoint_name(device).ok()?.trim().to_owned();
    (!name.is_empty()).then_some(AudioEndpointInfo { id, name })
}

fn endpoint_id(device: &IMMDevice) -> Result<String, WindowsError> {
    let value = unsafe { device.GetId() }?;
    let id = unsafe { value.to_string() };
    unsafe { CoTaskMemFree(Some(value.0.cast())) };
    Ok(id?)
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
