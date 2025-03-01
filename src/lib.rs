#![deny(missing_docs)]
#![doc(html_root_url = "http://docs.rs/ddc-hi-rs/0.3.0")]

//! High level DDC/CI monitor controls.
//!
//! # Example
//!
//! ```rust,no_run
//! use ddc_hi::{Ddc, Display};
//!
//! for mut display in Display::enumerate() {
//!     display.update_capabilities().unwrap();
//!     println!("{:?} {}: {:?} {:?}",
//!        display.info.backend, display.info.id,
//!        display.info.manufacturer_id, display.info.model_name
//!     );
//!     if let Some(feature) = display.info.mccs_database.get(0xdf) {
//!         let value = display.handle.get_vcp_feature(feature.code).unwrap();
//!         println!("{}: {:?}", feature.name.as_ref().unwrap(), value);
//!     }
//! }
//! ```

#[macro_use]
extern crate log;
#[cfg(feature = "has-ddc-i2c")]
extern crate ddc_i2c;
#[cfg(feature = "has-ddc-macos")]
extern crate ddc_macos;
#[cfg(feature = "has-ddc-winapi")]
extern crate ddc_winapi;
#[cfg(feature = "has-nvapi")]
extern crate nvapi;

use ddc::Edid;
use std::iter::FromIterator;
use std::{fmt, io, str};
use thiserror::Error;

pub use ddc::{Ddc, DdcHost, DdcTable, FeatureCode, TimingMessage, VcpValue, VcpValueType};

/// Generc DDC/CI error
#[derive(Error, Debug)]
pub enum DdcCiError {
    /// Capabilities couldn't be parsed
    #[error("Failed parsing capabilities")]
    ParseCapabilities(#[from] io::Error),

    #[cfg(feature = "has-ddc-i2c")]
    /// There was an error in the underlying I²C communication
    #[error("Error during DDC/CI I²C communication")]
    DdcI2cIoError(#[from] ddc_i2c::Error<io::Error>),

    #[cfg(feature = "has-nvapi")]
    /// There was an error in the underlying I²C communication
    #[error("Error during DDC/CI I²C communication")]
    DdcI2cStatusError(#[from] ddc_i2c::Error<nvapi::Status>),

    #[cfg(feature = "has-nvapi")]
    /// There was an error in the nvapi code
    #[error("NVAPI status error")]
    StatusError(#[from] nvapi::Status),
}

/// Identifying information about an attached display.
///
/// Not all information will be available, particularly on backends like
/// WinAPI that do not support EDID.
//#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[derive(Clone, Debug)]
pub struct DisplayInfo {
    /// Identifies the backend or driver used to communicate with the display.
    pub backend: Backend,
    /// A unique identifier for the display, format is specific to the backend.
    pub id: String,
    /// A three-character identifier of the manufacturer of the display.
    pub manufacturer_id: Option<String>,
    /// A number that identifies the product model.
    pub model_id: Option<u16>,
    /// The version and revision of the product.
    pub version: Option<(u8, u8)>,
    /// Serial number of the device
    pub serial: Option<u32>,
    /// Year the display was manufactured.
    pub manufacture_year: Option<u8>,
    /// Week the display was manufactured.
    pub manufacture_week: Option<u8>,
    /// The model name of the display.
    pub model_name: Option<String>,
    /// Human-readable serial number of the device.
    pub serial_number: Option<String>,
    /// Raw EDID data provided by the display.
    pub edid_data: Option<Vec<u8>>,
    /// MCCS VCP version code.
    pub mccs_version: Option<mccs::Version>,
    /// MCCS VCP feature information.
    pub mccs_database: mccs_db::Database,
}

impl DisplayInfo {
    /// Create an empty `DisplayInfo`.
    pub fn new(backend: Backend, id: String) -> Self {
        DisplayInfo {
            backend,
            id,
            manufacturer_id: None,
            model_id: None,
            version: None,
            serial: None,
            manufacture_year: None,
            manufacture_week: None,
            model_name: None,
            serial_number: None,
            edid_data: None,
            mccs_version: None,
            mccs_database: Default::default(),
        }
    }

    /// Creates a new `DisplayInfo` from unparsed EDID data.
    ///
    /// May fail to parse the EDID data.
    pub fn from_edid(backend: Backend, id: String, edid_data: Vec<u8>) -> io::Result<Self> {
        trace!("DisplayInfo::from_edid({:?}, {})", backend, id);

        let edid = edid::parse(&edid_data)
            .to_result()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        let mut model_name = None;
        let mut serial_number = None;

        for desc in edid.descriptors {
            match desc {
                edid::Descriptor::SerialNumber(serial) => serial_number = Some(serial),
                edid::Descriptor::ProductName(model) => model_name = Some(model),
                _ => (),
            }
        }

        Ok(DisplayInfo {
            backend,
            id,
            edid_data: Some(edid_data),
            manufacturer_id: Some(String::from_iter(edid.header.vendor.iter())),
            model_id: Some(edid.header.product),
            serial: Some(edid.header.serial),
            version: Some((edid.header.version, edid.header.revision)),
            manufacture_year: Some(edid.header.year),
            manufacture_week: Some(edid.header.week),
            model_name,
            serial_number,
            mccs_version: None,
            mccs_database: Default::default(),
        })
    }

    /// Create a new `DisplayInfo` from parsed capabilities.
    pub fn from_capabilities(backend: Backend, id: String, caps: &mccs::Capabilities) -> Self {
        trace!("DisplayInfo::from_capabilities({:?}, {})", backend, id);

        let edid = if let Some(ref edid) = caps.edid {
            // TODO: return Result here? warn!()?
            Self::from_edid(backend, id.clone(), edid.clone()).map_err(drop)
        } else {
            Err(())
        };

        let mut res = DisplayInfo {
            backend,
            id,
            model_name: caps.model.clone(),
            mccs_version: caps.mccs_version,
            edid_data: caps.edid.clone(),
            // TODO: VDIF
            serial_number: None,
            manufacturer_id: None,
            model_id: None,
            serial: None,
            version: None,
            manufacture_year: None,
            manufacture_week: None,
            mccs_database: Default::default(),
        };

        if let Some(ver) = res.mccs_version.as_ref() {
            res.mccs_database = mccs_db::Database::from_version(ver);
            res.mccs_database.apply_capabilities(caps);
        }

        if let Ok(edid) = edid {
            // TODO: should this be edid.update_from(&res) instead?
            res.update_from(&edid);
        }

        res
    }

    /// Merge in any missing information from another `DisplayInfo`
    pub fn update_from(&mut self, info: &DisplayInfo) {
        if self.manufacturer_id.is_none() {
            self.manufacturer_id = info.manufacturer_id.clone()
        }

        if self.model_id.is_none() {
            self.model_id = info.model_id
        }

        if self.version.is_none() {
            self.version = info.version
        }

        if self.serial.is_none() {
            self.serial = info.serial
        }

        if self.manufacture_year.is_none() {
            self.manufacture_year = info.manufacture_year
        }

        if self.manufacture_week.is_none() {
            self.manufacture_week = info.manufacture_week
        }

        if self.model_name.is_none() {
            self.model_name = info.model_name.clone()
        }

        if self.serial_number.is_none() {
            self.serial_number = info.serial_number.clone()
        }

        if self.edid_data.is_none() {
            self.edid_data = info.edid_data.clone()
        }

        if self.mccs_version.is_none() {
            self.mccs_version = info.mccs_version
        }

        if self.mccs_database.get(0xdf).is_none() {
            if info.mccs_version.is_some() {
                self.mccs_version = info.mccs_version
            }
            self.mccs_database = info.mccs_database.clone()
        }
    }

    /// Populate information from a DDC connection.
    ///
    /// This will read the VCP Version (`0xdf`) and fill in the `mccs_database`.
    /// This data will be incomplete compared to being filled in from a capability
    /// string.
    pub fn update_from_ddc<D: Ddc>(&mut self, ddc: &mut D) -> Result<(), D::Error> {
        if self.mccs_version.is_none() {
            trace!("DisplayInfo::update_from_ddc");

            let version = ddc.get_vcp_feature(0xdf)?;
            let version = mccs::Version::new(version.sh, version.sl);
            if version != mccs::Version::default() {
                self.mccs_version = Some(version);
                self.mccs_database = mccs_db::Database::from_version(&version);
            }
        }

        Ok(())
    }
}

/// A query to filter out matching displays.
///
/// Most comparisons must match the full string.
#[derive(Debug)]
pub enum Query {
    /// Matches any display
    Any,
    /// Matches a display on the given backend
    Backend(Backend),
    /// Matches a display with the specified ID
    Id(String),
    /// Matches a display with the specified manufacturer
    ManufacturerId(String),
    /// Matches a display with the specified model name
    ModelName(String),
    /// Matches a display with the specified serial number
    SerialNumber(String),
    /// At least one of the queries must match
    Or(Vec<Query>),
    /// All of the queries must match
    And(Vec<Query>),
}

impl Query {
    /// Queries whether the provided display info is a match.
    pub fn matches(&self, info: &DisplayInfo) -> bool {
        match *self {
            Query::Any => true,
            Query::Backend(backend) => info.backend == backend,
            Query::Id(ref id) => &info.id == id,
            Query::ManufacturerId(ref id) => info.manufacturer_id.as_ref() == Some(id),
            Query::ModelName(ref model) => info.model_name.as_ref() == Some(model),
            Query::SerialNumber(ref serial) => info.serial_number.as_ref() == Some(serial),
            Query::Or(ref query) => query.iter().any(|q| q.matches(info)),
            Query::And(ref query) => query.iter().all(|q| q.matches(info)),
        }
    }
}

/// Identifies the backend driver used to communicate with a display.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[allow(clippy::upper_case_acronyms)]
pub enum Backend {
    /// Linux i2c-dev driver
    I2cDevice,
    /// Windows Monitor Configuration API
    WinApi,
    /// NVIDIA NVAPI driver
    Nvapi,
    /// MacOS APIs
    MacOS,
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match *self {
                Backend::I2cDevice => "i2c-dev",
                Backend::WinApi => "winapi",
                Backend::Nvapi => "nvapi",
                Backend::MacOS => "macos",
            }
        )
    }
}

/// Failed to parse backend
#[derive(Error, Debug)]
#[error("Failed parsing backend")]
pub struct ParseBackendError;

impl str::FromStr for Backend {
    type Err = ParseBackendError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "i2c-dev" => Backend::I2cDevice,
            "winapi" => Backend::WinApi,
            "nvapi" => Backend::Nvapi,
            "macos" => Backend::MacOS,
            _ => return Err(ParseBackendError),
        })
    }
}

impl Backend {
    /// Enumerate the possible backends.
    ///
    /// Backends not supported for the current platform will be excluded.
    pub const fn values() -> &'static [Backend] {
        &[
            #[cfg(feature = "has-ddc-i2c")]
            Backend::I2cDevice,
            #[cfg(feature = "has-ddc-winapi")]
            Backend::WinApi,
            #[cfg(feature = "has-nvapi")]
            Backend::Nvapi,
            #[cfg(feature = "has-ddc-macos")]
            Backend::MacOS,
        ]
    }
    /// Enumerate the possible backends (as str's)
    ///
    /// Backends not supported for the current platform will be excluded.
    pub const fn values_str() -> &'static [&'static str] {
        &[
            #[cfg(feature = "has-ddc-i2c")]
            "i2c-dev",
            #[cfg(feature = "has-ddc-winapi")]
            "winapi",
            #[cfg(feature = "has-nvapi")]
            "nvapi",
            #[cfg(feature = "has-ddc-macos")]
            "macos",
        ]
    }
}

/// An active handle to a connected display.
pub struct Display {
    /// The inner communication handle used for DDC commands.
    pub handle: Handle,
    /// Information about the connected display.
    pub info: DisplayInfo,
    filled_caps: bool,
}

impl Display {
    /// Create a new display from the specified handle.
    pub const fn new(handle: Handle, info: DisplayInfo) -> Self {
        Display {
            handle,
            info,
            filled_caps: false,
        }
    }

    /// Enumerate all detected displays.
    pub fn enumerate() -> Vec<Self> {
        let mut displays = Vec::new();

        #[cfg(feature = "has-ddc-i2c")]
        {
            use std::os::unix::fs::MetadataExt;

            match ddc_i2c::I2cDeviceEnumerator::new() {
                Ok(devs) => displays.extend(
                    devs.map(|mut ddc: ddc_i2c::I2cDeviceDdc| -> Result<_, ()> {
                        let mut edid = vec![0u8; 0x100];
                        ddc.read_edid(0, &mut edid).map_err(|e| {
                            info!("Failed to read DDC edid: {}", e);
                        })?;
                        let id = ddc
                            .inner_ref()
                            .inner_ref()
                            .metadata()
                            .map(|meta| meta.rdev().to_string())
                            .unwrap_or_default();
                        let info =
                            DisplayInfo::from_edid(Backend::I2cDevice, id, edid).map_err(|e| {
                                info!("Failed to parse display info from edid: {}", e);
                            })?;
                        Ok(Display::new(Handle::I2cDevice(ddc), info))
                    })
                    .filter_map(|d| d.ok()),
                ),
                Err(e) => info!("Error during I2C device enumeration: {}", e),
            }
        }

        #[cfg(feature = "has-ddc-winapi")]
        {
            if let Ok(devs) = ddc_winapi::Monitor::enumerate() {
                displays.extend(devs.into_iter().map(|ddc| {
                    let info = DisplayInfo::new(Backend::WinApi, ddc.description());
                    Display::new(Handle::WinApi(ddc), info)
                }))
            }
        }

        #[cfg(feature = "has-ddc-macos")]
        {
            if let Ok(devs) = ddc_macos::Monitor::enumerate() {
                displays.extend(devs.into_iter().map(|ddc| {
                    let info = ddc
                        .edid()
                        .and_then(|edid| {
                            DisplayInfo::from_edid(Backend::MacOS, ddc.description(), edid).ok()
                        })
                        .unwrap_or(DisplayInfo::new(Backend::MacOS, ddc.description()));
                    Display::new(Handle::MacOS(ddc), info)
                }))
            }
        }

        #[cfg(feature = "has-nvapi")]
        {
            use std::rc::Rc;

            if let Ok(_) = nvapi::initialize() {
                if let Ok(gpus) = nvapi::PhysicalGpu::enumerate() {
                    for gpu in gpus {
                        let gpu = Rc::new(gpu);
                        let id_prefix = gpu.short_name().unwrap_or("NVAPI".into());
                        if let Ok(ids) =
                            gpu.display_ids_connected(nvapi::ConnectedIdsFlags::empty())
                        {
                            for id in ids {
                                let mut i2c = nvapi::I2c::new(gpu.clone(), id.display_id); // TODO: it says mask, is it actually `1<<display_id` instead?
                                i2c.set_port(None, true); // TODO: port=Some(1) instead? docs seem to indicate it's not optional, but the one example I can find keeps it unset so...

                                // hack around broken nvidia drivers, the register argument doesn't seem to work at all so write the edid eeprom offset here first
                                i2c.set_address(0x50);
                                let _ = i2c.nvapi_write(&[], &[0]);

                                let mut ddc = ddc_i2c::I2cDdc::new(i2c);

                                let id =
                                    format!("{}/{}:{:?}", id_prefix, id.display_id, id.connector);
                                let mut edid = vec![0u8; 0x80]; // 0x100
                                if let Ok(ddc) = ddc
                                    .read_edid(0, &mut edid)
                                    .map_err(DdcCiError::StatusError)
                                    .and_then(|_| {
                                        DisplayInfo::from_edid(Backend::Nvapi, id, edid)
                                            .map_err(DdcCiError::from)
                                    })
                                    .map(|info| Display::new(Handle::Nvapi(ddc), info))
                                {
                                    displays.push(ddc);
                                }
                            }
                        }
                    }
                }
            }
        }

        displays
    }

    /// Updates the display info with data retrieved from the device's
    /// reported capabilities.
    pub fn update_capabilities(&mut self) -> Result<(), DdcCiError> {
        if !self.filled_caps {
            let (backend, id) = (self.info.backend, self.info.id.clone());
            let caps = self.handle.capabilities()?;
            let info = DisplayInfo::from_capabilities(backend, id, &caps);
            if info.mccs_version.is_some() {
                self.info.mccs_database = Default::default();
            }
            self.info.update_from(&info);
        }

        Ok(())
    }

    /// Update some display info.
    pub fn update_from_ddc(&mut self) -> Result<(), DdcCiError> {
        self.info.update_from_ddc(&mut self.handle)
    }
}

/// A handle allowing communication with a display
pub enum Handle {
    #[doc(hidden)]
    #[cfg(feature = "has-ddc-i2c")]
    I2cDevice(ddc_i2c::I2cDeviceDdc),
    #[doc(hidden)]
    #[cfg(feature = "has-ddc-winapi")]
    WinApi(ddc_winapi::Monitor),
    #[doc(hidden)]
    #[cfg(feature = "has-ddc-macos")]
    MacOS(ddc_macos::Monitor),
    #[doc(hidden)]
    #[cfg(feature = "has-nvapi")]
    Nvapi(ddc_i2c::I2cDdc<nvapi::I2c<::std::rc::Rc<nvapi::PhysicalGpu>>>),
}

impl Handle {
    /// Request and parse the display's capabilities string.
    pub fn capabilities(&mut self) -> Result<mccs::Capabilities, DdcCiError> {
        mccs_caps::parse_capabilities(&self.capabilities_string()?)
            .map_err(DdcCiError::ParseCapabilities)
    }
}

impl ddc::DdcHost for Handle {
    type Error = DdcCiError;

    fn sleep(&mut self) {
        match *self {
            #[cfg(feature = "has-ddc-i2c")]
            Handle::I2cDevice(ref mut i2c) => i2c.sleep(),
            #[cfg(feature = "has-ddc-winapi")]
            Handle::WinApi(ref mut monitor) => monitor.sleep(),
            #[cfg(feature = "has-ddc-macos")]
            Handle::MacOS(ref mut monitor) => monitor.sleep(),
            #[cfg(feature = "has-nvapi")]
            Handle::Nvapi(ref mut i2c) => i2c.sleep(),
        }
    }
}

impl Ddc for Handle {
    fn capabilities_string(&mut self) -> Result<Vec<u8>, Self::Error> {
        match *self {
            #[cfg(feature = "has-ddc-i2c")]
            Handle::I2cDevice(ref mut i2c) => i2c.capabilities_string().map_err(From::from),
            #[cfg(feature = "has-ddc-winapi")]
            Handle::WinApi(ref mut monitor) => monitor.capabilities_string().map_err(From::from),
            #[cfg(feature = "has-ddc-macos")]
            Handle::MacOS(ref mut monitor) => monitor.capabilities_string().map_err(From::from),
            #[cfg(feature = "has-nvapi")]
            Handle::Nvapi(ref mut i2c) => i2c.capabilities_string().map_err(From::from),
        }
    }

    fn get_vcp_feature(&mut self, code: FeatureCode) -> Result<VcpValue, Self::Error> {
        match *self {
            #[cfg(feature = "has-ddc-i2c")]
            Handle::I2cDevice(ref mut i2c) => i2c.get_vcp_feature(code).map_err(From::from),
            #[cfg(feature = "has-ddc-winapi")]
            Handle::WinApi(ref mut monitor) => monitor.get_vcp_feature(code).map_err(From::from),
            #[cfg(feature = "has-ddc-macos")]
            Handle::MacOS(ref mut monitor) => monitor.get_vcp_feature(code).map_err(From::from),
            #[cfg(feature = "has-nvapi")]
            Handle::Nvapi(ref mut i2c) => i2c.get_vcp_feature(code).map_err(From::from),
        }
    }

    fn set_vcp_feature(&mut self, code: FeatureCode, value: u16) -> Result<(), Self::Error> {
        match *self {
            #[cfg(feature = "has-ddc-i2c")]
            Handle::I2cDevice(ref mut i2c) => i2c.set_vcp_feature(code, value).map_err(From::from),
            #[cfg(feature = "has-ddc-winapi")]
            Handle::WinApi(ref mut monitor) => {
                monitor.set_vcp_feature(code, value).map_err(From::from)
            }
            #[cfg(feature = "has-ddc-macos")]
            Handle::MacOS(ref mut monitor) => {
                monitor.set_vcp_feature(code, value).map_err(From::from)
            }
            #[cfg(feature = "has-nvapi")]
            Handle::Nvapi(ref mut i2c) => i2c.set_vcp_feature(code, value).map_err(From::from),
        }
    }

    fn save_current_settings(&mut self) -> Result<(), Self::Error> {
        match *self {
            #[cfg(feature = "has-ddc-i2c")]
            Handle::I2cDevice(ref mut i2c) => i2c.save_current_settings().map_err(From::from),
            #[cfg(feature = "has-ddc-winapi")]
            Handle::WinApi(ref mut monitor) => monitor.save_current_settings().map_err(From::from),
            #[cfg(feature = "has-ddc-macos")]
            Handle::MacOS(ref mut monitor) => monitor.save_current_settings().map_err(From::from),
            #[cfg(feature = "has-nvapi")]
            Handle::Nvapi(ref mut i2c) => i2c.save_current_settings().map_err(From::from),
        }
    }

    fn get_timing_report(&mut self) -> Result<TimingMessage, Self::Error> {
        match *self {
            #[cfg(feature = "has-ddc-i2c")]
            Handle::I2cDevice(ref mut i2c) => i2c.get_timing_report().map_err(From::from),
            #[cfg(feature = "has-ddc-winapi")]
            Handle::WinApi(ref mut monitor) => monitor.get_timing_report().map_err(From::from),
            #[cfg(feature = "has-ddc-macos")]
            Handle::MacOS(ref mut monitor) => monitor.get_timing_report().map_err(From::from),
            #[cfg(feature = "has-nvapi")]
            Handle::Nvapi(ref mut i2c) => i2c.get_timing_report().map_err(From::from),
        }
    }
}

impl DdcTable for Handle {
    #[cfg_attr(feature = "has-ddc-winapi", allow(unused_variables))]
    fn table_read(&mut self, code: FeatureCode) -> Result<Vec<u8>, Self::Error> {
        match *self {
            #[cfg(feature = "has-ddc-i2c")]
            Handle::I2cDevice(ref mut i2c) => i2c.table_read(code).map_err(From::from),
            #[cfg(feature = "has-ddc-macos")]
            Handle::MacOS(ref mut i2c) => i2c.table_read(code).map_err(From::from),
            #[cfg(feature = "has-ddc-winapi")]
            Handle::WinApi(_) => Err(io::Error::new(
                io::ErrorKind::Other,
                "winapi does not support DDC tables",
            )
            .into()),
            #[cfg(feature = "has-nvapi")]
            Handle::Nvapi(ref mut i2c) => i2c.table_read(code).map_err(From::from),
        }
    }

    #[cfg_attr(feature = "has-ddc-winapi", allow(unused_variables))]
    fn table_write(
        &mut self,
        code: FeatureCode,
        offset: u16,
        value: &[u8],
    ) -> Result<(), Self::Error> {
        match *self {
            #[cfg(feature = "has-ddc-i2c")]
            Handle::I2cDevice(ref mut i2c) => {
                i2c.table_write(code, offset, value).map_err(From::from)
            }
            #[cfg(feature = "has-ddc-macos")]
            Handle::MacOS(ref mut i2c) => i2c.table_write(code, offset, value).map_err(From::from),
            #[cfg(feature = "has-ddc-winapi")]
            Handle::WinApi(_) => Err(io::Error::new(
                io::ErrorKind::Other,
                "winapi does not support DDC tables",
            )
            .into()),
            #[cfg(feature = "has-nvapi")]
            Handle::Nvapi(ref mut i2c) => i2c.table_write(code, offset, value).map_err(From::from),
        }
    }
}
