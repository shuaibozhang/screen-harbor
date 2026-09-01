use windows::{
    core::Error as WindowsError,
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{
            CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
        },
    },
};

pub(crate) struct ComApartment {
    should_uninitialize: bool,
}

impl ComApartment {
    pub(crate) fn enter_multithreaded() -> Result<Self, WindowsError> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        match result {
            result if result.is_ok() => Ok(Self {
                should_uninitialize: true,
            }),
            RPC_E_CHANGED_MODE => Ok(Self {
                should_uninitialize: false,
            }),
            error => Err(error.into()),
        }
    }

    pub(crate) fn enter_apartment_threaded() -> Result<Self, WindowsError> {
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        match result {
            result if result.is_ok() => Ok(Self {
                should_uninitialize: true,
            }),
            RPC_E_CHANGED_MODE => Ok(Self {
                should_uninitialize: false,
            }),
            error => Err(error.into()),
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.should_uninitialize {
            unsafe { CoUninitialize() };
        }
    }
}
