//! Rust Windows resource helper
//!
//! This crate implements a simple generator for Windows resource (.rc) files
//! for use with either Microsoft `rc.exe` resource compiler or with GNU `windres.exe`
//!
//! The [`WindowsResorce::compile()`] method is intended to be used from a build script and
//! needs environment variables from cargo to be set. It not only compiles the resource
//! but directs cargo to link the resource compiler's output.
//!
//! # Example
//!
//! ```rust
//! # extern crate winres;
//! # use std::io;
//! # fn test_main() -> io::Result<()> {
//! if cfg!(target_os = "windows") {
//!     let mut res = winres::WindowsResource::new();
//!     res.set_icon("test.ico")
//! #      .set_output_directory(".")
//!        .set("InternalName", "TEST.EXE")
//!        // manually set version 1.0.0.0
//!        .set_version_info(winres::VersionInfo::PRODUCTVERSION, 0x0001000000000000);
//!     res.compile()?;
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Defaults
//!
//! We try to guess some sensible default values from Cargo's build time environement variables
//! This is described in [`WindowsResource::new()`]. Furthermore we have to know where to find the
//! resource compiler for the MSVC Toolkit. This can be done by looking up a registry key but
//! for MinGW this has to be done manually.
//!
//! The following paths are the hardcoded defaults:
//! MSVC the last registry key at
//! `HKLM\SOFTWARE\Microsoft\Windows Kits\Installed Roots`, for MinGW we try our luck by simply
//! using the `%PATH%` environment variable.
//!
//! Note that the toolkit bitness as to match the one from the current Rust compiler. If you are
//! using Rust GNU 64-bit you have to use MinGW64. For MSVC this is simpler as (recent) Windows
//! SDK always installs both versions on a 64-bit system.
//!
//! [`WindowsResorce::compile()`]: struct.WindowsResource.html#method.compile
//! [`WindowsResource::new()`]: struct.WindowsResource.html#method.new

use std::env;
use std::path::{PathBuf, Path};
use std::process;
use std::collections::HashMap;
use std::io;
use std::io::prelude::*;
use std::fs;
use std::error::Error;

extern crate toml;

pub mod sdk;

/// Version info field names
#[derive(PartialEq, Eq, Hash, Debug)]
pub enum VersionInfo {
    /// The version value consists of four 16 bit words, e.g.,
    /// `MAJOR << 48 | MINOR << 32 | PATCH << 16 | RELEASE`
    FILEVERSION,
    /// The version value consists of four 16 bit words, e.g.,
    /// `MAJOR << 48 | MINOR << 32 | PATCH << 16 | RELEASE`
    PRODUCTVERSION,
    /// Should be Windows NT Win32, with value `0x40004`
    FILEOS,
    /// The value (for a rust compiler output) should be
    /// 1 for a EXE and 2 for a DLL
    FILETYPE,
    /// Only for Windows drivers
    FILESUBTYPE,
    /// Bit mask for FILEFLAGS
    FILEFLAGSMASK,
    /// Only the bits set in FILEFLAGSMASK are read
    FILEFLAGS,
}

#[derive(Debug)]
pub struct WindowsResource {
    tool: sdk::Tool,
    properties: HashMap<String, String>,
    version_info: HashMap<VersionInfo, u64>,
    rc_file: Option<String>,
    icon_id: Option<String>,
    icon: Option<String>,
    language: u16,
    manifest: Option<String>,
    manifest_file: Option<String>,
    output_directory: String,
    windres_path: Option<String>,
    ar_path: Option<String>,
}

impl WindowsResource {

    /// Create a new resource with version info struct
    ///
    ///
    /// We initialize the resource file with values provided by cargo
    ///
    /// | Field                | Cargo / Values               |
    /// |----------------------|------------------------------|
    /// | `"FileVersion"`      | `package.version`            |
    /// | `"ProductVersion"`   | `package.version`            |
    /// | `"ProductName"`      | `package.name`               |
    /// | `"FileDescription"`  | `package.description`        |
    ///
    /// Furthermore if a section `package.metadata.winres` exists
    /// in `Cargo.toml` it will be parsed. Values in this section take precedence
    /// over the values provided natively by cargo. Only the string table
    /// of the version struct can be set this way.
    /// Additionally, the language field is set to neutral (i.e. `0`)
    /// and no icon is set. These settings have to be done programmatically.
    ///
    /// `Cargo.toml` files have to be written in UTF-8, so we support all valid UTF-8 strings
    /// provided.
    ///
    /// ```,toml
    /// #Cargo.toml
    /// [package.metadata.winres]
    /// OriginalFilename = "testing.exe"
    /// FileDescription = "⛄❤☕"
    /// LegalCopyright = "Copyright © 2016"
    /// ```
    ///
    /// The version info struct is set to some values
    /// sensible for creating an executable file.
    ///
    /// | Property             | Cargo / Values               |
    /// |----------------------|------------------------------|
    /// | `FILEVERSION`        | `package.version`            |
    /// | `PRODUCTVERSION`     | `package.version`            |
    /// | `FILEOS`             | `VOS_NT_WINDOWS32 (0x40004)` |
    /// | `FILETYPE`           | `VFT_APP (0x1)`              |
    /// | `FILESUBTYPE`        | `VFT2_UNKNOWN (0x0)`         |
    /// | `FILEFLAGSMASK`      | `VS_FFI_FILEFLAGSMASK (0x3F)`|
    /// | `FILEFLAGS`          | `0x0`                        |
    ///
    pub fn new() -> Self {
        let mut props: HashMap<String, String> = HashMap::new();
        let mut ver: HashMap<VersionInfo, u64> = HashMap::new();

        props.insert("FileVersion".to_string(),
                     env::var("CARGO_PKG_VERSION").expect("env").to_string());
        props.insert("ProductVersion".to_string(),
                     env::var("CARGO_PKG_VERSION").expect("env").to_string());
        props.insert("ProductName".to_string(),
                     env::var("CARGO_PKG_NAME").expect("env").to_string());
        props.insert("FileDescription".to_string(),
                     env::var("CARGO_PKG_DESCRIPTION").expect("env").to_string());

        parse_cargo_toml(&mut props).expect("parse toml");

        let mut version = 0 as u64;
        version |= env::var("CARGO_PKG_VERSION_MAJOR").expect("env").parse().unwrap_or(0) << 48;
        version |= env::var("CARGO_PKG_VERSION_MINOR").expect("env").parse().unwrap_or(0) << 32;
        version |= env::var("CARGO_PKG_VERSION_PATCH").expect("env").parse().unwrap_or(0) << 16;
        // version |= env::var("CARGO_PKG_VERSION_PRE").unwrap().parse().unwrap_or(0);
        ver.insert(VersionInfo::FILEVERSION, version);
        ver.insert(VersionInfo::PRODUCTVERSION, version);
        ver.insert(VersionInfo::FILEOS, 0x00040004);
        ver.insert(VersionInfo::FILETYPE, 1);
        ver.insert(VersionInfo::FILESUBTYPE, 0);
        ver.insert(VersionInfo::FILEFLAGSMASK, 0x3F);
        ver.insert(VersionInfo::FILEFLAGS, 0);

        let tool = if cfg!(target_env = "msvc") {
            get_sdk().expect("get_sdk")
        } else if cfg!(target_os = "windows") {
            unimplemented!()
        } else {
            unimplemented!()
        };

        WindowsResource {
            tool: tool,
            properties: props,
            version_info: ver,
            rc_file: None,
            icon_id: None,
            icon: None,
            language: 0,
            manifest: None,
            manifest_file: None,
            output_directory: env::var("OUT_DIR").unwrap_or(".".to_string()),
            windres_path: None,
            ar_path: None,
        }
    }

    /// Set string properties of the version info struct.
    ///
    /// Possible field names are:
    ///
    ///  - `"FileVersion"`
    ///  - `"FileDescription"`
    ///  - `"ProductVersion"`
    ///  - `"ProductName"`
    ///  - `"OriginalFilename"`
    ///  - `"LegalCopyright"`
    ///  - `"LegalTrademark"`
    ///  - `"CompanyName"`
    ///  - `"Comments"`
    ///  - `"InternalName"`
    ///
    /// Additionally there exists
    /// `"PrivateBuild"`, `"SpecialBuild"`
    /// which should only be set, when the `FILEFLAGS` property is set to
    /// `VS_FF_PRIVATEBUILD(0x08)` or `VS_FF_SPECIALBUILD(0x20)`
    ///
    /// It is possible to use arbirtrary field names but Windows Explorer and other
    /// tools might not show them.
    pub fn set<'a>(&mut self, name: &'a str, value: &'a str) -> &mut Self {
        self.properties.insert(name.to_string(), value.to_string());
        self
    }

    /// Set the correct tool.
    ///
    /// For the GNU toolkit this has to be the path where MinGW
    /// put `windres.exe` and `ar.exe`. This could be something like:
    /// `"C:\Program Files\mingw-w64\x86_64-5.3.0-win32-seh-rt_v4-rev0\mingw64\bin"`
    ///
    /// For MSVC the Windows SDK has to be installed. It comes with the resource compiler
    /// `rc.exe`. This should be set to the root directory of the Windows SDK, e.g.,
    /// `"C:\Program Files (x86)\Windows Kits\10"`
    /// or, if multiple 10 versions are installed,
    /// set it directly to the corret bin directory
    /// `"C:\Program Files (x86)\Windows Kits\10\bin\10.0.14393.0\x64"`
    ///
    /// If it is left unset, it will look up a path in the registry,
    /// i.e. `HKLM\SOFTWARE\Microsoft\Windows Kits\Installed Roots`
    pub fn set_tool<'a>(&mut self, tool: sdk::Tool) -> &mut Self {
        self.tool = tool;
        self
    }

    /// Set the user interface language of the file
    ///
    /// # Example
    ///
    /// ```
    /// extern crate winapi;
    /// extern crate winres;
    /// # use std::io;
    /// fn main() {
    ///   if cfg!(target_os = "windows") {
    ///     let mut res = winres::WindowsResource::new();
    /// #   res.set_output_directory(".");
    ///     res.set_language(winapi::um::winnt::MAKELANGID(
    ///         winapi::um::winnt::LANG_ENGLISH,
    ///         winapi::um::winnt::SUBLANG_ENGLISH_US
    ///     ));
    ///     res.compile().expect("compile");
    ///   }
    /// }
    /// ```
    /// For possible values look at the `winapi::um::winnt` constants, specifically those
    /// starting with `LANG_` and `SUBLANG_`.
    ///
    /// [`MAKELANGID`]: https://docs.rs/winapi/0.3/x86_64-pc-windows-msvc/winapi/um/winnt/fn.MAKELANGID.html
    /// [`winapi::um::winnt`]: https://docs.rs/winapi/0.3/x86_64-pc-windows-msvc/winapi/um/winnt/index.html#constants
    ///
    /// # Table
    /// Sometimes it is just simpler to specify the numeric constant directly
    /// (That is what most `.rc` files do).
    /// For possible values take a look at the MSDN page for resource files;
    /// we only listed some values here.
    ///
    /// | Language            | Value    |
    /// |---------------------|----------|
    /// | Neutral             | `0x0000` |
    /// | English             | `0x0009` |
    /// | English (US)        | `0x0409` |
    /// | English (GB)        | `0x0809` |
    /// | German              | `0x0407` |
    /// | German (AT)         | `0x0c07` |
    /// | French              | `0x000c` |
    /// | French (FR)         | `0x040c` |
    /// | Catalan             | `0x0003` |
    /// | Basque              | `0x042d` |
    /// | Breton              | `0x007e` |
    /// | Scottish Gaelic     | `0x0091` |
    /// | Romansch            | `0x0017` |
    pub fn set_language(&mut self, language: u16) -> &mut Self {
        self.language = language;
        self
    }

    /// Set an icon filename
    ///
    /// This icon need to be in `ico` format. The filename can be absolute
    /// or relative to the projects root.
    pub fn set_icon<'a>(&mut self, path: &'a str) -> &mut Self {
        self.icon = Some(path.to_string());
        self
    }

    /// Set an icon filename and icon id
    ///
    /// This icon need to be in `ico` format. The filename can be absolute
    /// or relative to the projects root.
    pub fn set_icon_with_id<'a>(&mut self, path: &'a str, icon_id: &'a str) -> &mut Self {
        self.icon = Some(path.to_string());
        self.icon_id = Some(icon_id.to_string());
        self
    }

    /// Set a version info struct property
    /// Currently we only support numeric values; you have to look them up.
    pub fn set_version_info(&mut self, field: VersionInfo, value: u64) -> &mut Self {
        self.version_info.insert(field, value);
        self
    }

    /// Set the embedded manifest file
    ///
    /// # Example
    ///
    /// The following manifest will brand the exe as requesting administrator privileges.
    /// Thus, everytime it is executed, a Windows UAC dialog will appear.
    ///
    /// ```rust
    /// let mut res = winres::WindowsResource::new();
    /// res.set_manifest(r#"
    /// <assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
    /// <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    ///     <security>
    ///         <requestedPrivileges>
    ///             <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
    ///         </requestedPrivileges>
    ///     </security>
    /// </trustInfo>
    /// </assembly>
    /// "#);
    /// ```
    pub fn set_manifest<'a>(&mut self, manifest: &'a str) -> &mut Self {
        self.manifest_file = None;
        self.manifest = Some(manifest.to_string());
        self
    }

    /// Some as [`set_manifest()`] but a filename can be provided and
    /// file is included by the resource compieler itself.
    /// This method works the same way as [`set_icon()`]
    ///
    /// [`set_manifest()`]: #method.set_manifest
    /// [`set_icon()`]: #method.set_icon
    pub fn set_manifest_file<'a>(&mut self, file: &'a str) -> &mut Self {
        self.manifest_file = Some(file.to_string());
        self.manifest = None;
        self
    }

    /// Set the path to the windres executable.
    pub fn set_windres_path(&mut self, path: &str) -> &mut Self {
        self.windres_path = Some(path.to_string());
        self
    }

    /// Set the path to the ar executable.
    pub fn set_ar_path(&mut self, path: &str) -> &mut Self {
        self.ar_path = Some(path.to_string());
        self
    }

    /// Write a resource file with the set values
    pub fn write_resource_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let mut f = try!(fs::File::create(path));
        // we don't need to include this, we use constants instead of macro names
        // try!(write!(f, "#include <winver.h>\n"));

        // use UTF8 as an encoding
        // this makes it easier since in rust all string are UTF8
        writeln!(f, "#pragma code_page(65001)")?;
        writeln!(f, "1 VERSIONINFO")?;
        for (k, v) in self.version_info.iter() {
            match *k {
                VersionInfo::FILEVERSION |
                VersionInfo::PRODUCTVERSION => {
                    writeln!(f,
                                  "{:?} {}, {}, {}, {}",
                                  k,
                                  (*v >> 48) as u16,
                                  (*v >> 32) as u16,
                                  (*v >> 16) as u16,
                                  *v as u16)?
                }
                _ => writeln!(f, "{:?} {:#x}", k, v)?,
            };
        }
        writeln!(f, "{{\nBLOCK \"StringFileInfo\"")?;
        writeln!(f, "{{\nBLOCK \"{:04x}04b0\"\n{{", self.language)?;
        for (k, v) in self.properties.iter() {
            if !v.is_empty() {
                writeln!(f, "VALUE \"{}\", \"{}\"",
                         escape_string(k), escape_string(v))?;
            }
        }
        writeln!(f, "}}\n}}")?;

        writeln!(f, "BLOCK \"VarFileInfo\" {{")?;
        writeln!(f, "VALUE \"Translation\", {:#x}, 0x04b0", self.language)?;
        writeln!(f, "}}\n}}")?;
        if let Some(ref icon) = self.icon {
            let name_id = self.icon_id.as_ref().map(String::as_str).unwrap_or("1");
            writeln!(f, "{} ICON \"{}\"", escape_string(name_id), escape_string(icon))?;
        }
        if let Some(e) = self.version_info.get(&VersionInfo::FILETYPE) {
            if let Some(manf) = self.manifest.as_ref() {
                writeln!(f, "{} 24", e)?;
                writeln!(f, "{{")?;
                for line in manf.lines() {
                    writeln!(f, "\"{}\"", escape_string(line.trim()))?;
                }
                writeln!(f, "}}")?;
            } else if let Some(manf) = self.manifest_file.as_ref() {
                writeln!(f, "{} 24 \"{}\"", e, escape_string(manf))?;
            }
        }
        Ok(())
    }

    /// Set a path to an already existing resource file.
    ///
    /// We will neither modify this file nor parse its contents. This function
    /// simply replaces the internaly generated resource file that is passed to
    /// the compiler. You can use this function to write a resource file yourself.
    pub fn set_resource_file<'a>(&mut self, path: &'a str) -> &mut Self {
        self.rc_file = Some(path.to_string());
        self
    }

    /// Override the output directoy.
    ///
    /// As a default, we use `%OUT_DIR%` set by cargo, but it may be necessary to override the
    /// the setting.
    pub fn set_output_directory<'a>(&mut self, path: &'a str) -> &mut Self {
        self.output_directory = path.to_string();
        self
    }

    #[cfg(target_env = "gnu")]
    fn compile_with_toolkit<'a>(&self, input: &'a str, output_dir: &'a str) -> io::Result<()> {
        let output = PathBuf::from(output_dir).join("resource.o");
        let input = PathBuf::from(input);
        let windres_path = self.windres_path.as_ref().map_or("windres.exe", String::as_str);
        let status = process::Command::new(windres_path)
            .current_dir(&self.toolkit_path)
            .arg(format!("-I{}", env::var("CARGO_MANIFEST_DIR").expect("env")))
            .arg(format!("{}", input.display()))
            .arg(format!("{}", output.display()))
            .status()?;
        if !status.success() {
            return Err(io::Error::new(io::ErrorKind::Other, "Could not compile resource file"));
        }

        let libname = PathBuf::from(output_dir).join("libresource.a");
        let ar_path = self.ar_path.as_ref().map_or("ar.exe", String::as_str);
        let status = process::Command::new(ar_path)
            .current_dir(&self.toolkit_path)
            .arg("rsc")
            .arg(format!("{}", libname.display()))
            .arg(format!("{}", output.display()))
            .status()?;
        if !status.success() {
            return Err(io::Error::new(io::ErrorKind::Other,
                                      "Could not create static library for resource file"));
        }

        println!("cargo:rustc-link-search=native={}", output_dir);
        println!("cargo:rustc-link-lib=static={}", "resource");

        Ok(())
    }

    /// Run the resource compiler
    ///
    /// This function generates a resource file from the settings or
    /// uses an existing resource file and passes it to the resource compiler
    /// of your toolkit.
    ///
    /// Further more we will print the correct statements for
    /// `cargo:rustc-link-lib=` and `cargo:rustc-link-search` on the console,
    /// so that the cargo build script can link the compiled resource file.
    pub fn compile(&self) -> io::Result<()> {
        let output = PathBuf::from(&self.output_directory);
        let rc = output.join("resource.rc");
        if self.rc_file.is_none() {
            self.write_resource_file(&rc)?;
        }
        let rc = if let Some(s) = self.rc_file.as_ref() {
            s.clone()
        } else {
            rc.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "utf8 decode"))?.to_string()
        };
        self.compile_with_toolkit(rc.as_str(), &self.output_directory)?;

        Ok(())
    }

    pub fn tool_path<'a>(&'a self) -> io::Result<&'a Path> {
        Ok(&self.tool.path)
    }

    pub fn include_dirs<'a>(&'a self) -> Vec<&Path> {
        self.tool.include_dirs.values().map(PathBuf::as_path).collect()
    }

    #[cfg(target_env = "msvc")]
    fn compile_with_toolkit<'a>(&self, input: &'a str, output_dir: &'a str) -> io::Result<()> {
        let rc_exe = self.tool_path()?;

        let output = PathBuf::from(output_dir).join("resource.lib");
        let input = PathBuf::from(input);

        let mut args = vec![];
        args.push(format!("/I{}", env::var("CARGO_MANIFEST_DIR").expect("env")));
        
        for inc in self.include_dirs() {
            args.push(format!("/I{}", inc.to_str().ok_or_else(||
                    io::Error::new(io::ErrorKind::Other, "unicode serialisation"))?));
        }

        args.push(format!("/fo{}", output.display()));
        args.push(format!("{}", input.display()));

        let status = process::Command::new(rc_exe)
            .args(&args)
            .output()?;
        
        println!("RC Output:\n{}\n------", String::from_utf8_lossy(&status.stdout));
        println!("RC Error:\n{}\n------", String::from_utf8_lossy(&status.stderr));
        if !status.status.success() {
            return Err(io::Error::new(io::ErrorKind::Other, "Could not compile resource file"));
        }

        println!("cargo:rustc-link-search=native={}", output_dir);
        println!("cargo:rustc-link-lib=dylib={}", "resource");
        Ok(())
    }

    #[cfg(not(any(target_env = "gnu", target_env = "msvc")))]
    fn compile_with_toolkit<'a>(&self, _input: &'a str, _output_dir: &'a str) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Other, "Can only compile resource file when target_env is \"gnu\" or \"msvc\""))
    }
}

/// Find a Windows SDK
fn get_sdk() -> io::Result<sdk::Tool> {
    // use the reg command, so we don't need a winapi dependency
    let system = sdk::System::new()?;
    let env_version = env::var("WindowsSDKVersion").ok();
    let arch = sdk::Arch::arch_for_cfg_target()
        .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "unsupported target arch"))?;
    let tools =  system.sdks.iter().filter_map(|sdk| sdk.tool("rc.exe", arch)).collect::<Vec<_>>();

    let max_version = tools.iter().max_by(|a,b| a.sdk_version.cmp(&b.sdk_version));

    let tool = tools.iter().find(|tool| {
        env_version.as_ref().map(|ev| ev == &tool.sdk_version).unwrap_or(false)
    }).or_else(|| max_version);

    tool.ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, format!("no rc.exe tool found for arch {} in {:?}", arch, system.installed_roots))
    }).map(std::borrow::ToOwned::to_owned)
}

fn parse_cargo_toml(props: &mut HashMap<String, String>) -> io::Result<()> {
    let cargo = Path::new(&env::var("CARGO_MANIFEST_DIR").expect("env")).join("Cargo.toml");
    let mut f = fs::File::open(cargo)?;
    let mut cargo_toml = String::new();
    f.read_to_string(&mut cargo_toml)?;
    if let Ok(ml) = cargo_toml.parse::<toml::Value>() {
        if let Some(pkg) = ml.get("package") {
            if let Some(pkg) = pkg.get("metadata") {
                if let Some(pkg) = pkg.get("winres") {
                    if let Some(pkg) = pkg.as_table() {
                        for (k, v) in pkg {
                            // println!("{} {}", k ,v);
                            if let Some(v) = v.as_str() {
                                props.insert(k.clone(), v.to_string());
                            } else {
                                println!("package.metadata.winres.{} is not a string", k);
                            }
                        }
                    } else {
                        println!("package.metadata.winres is not a table");
                    }
                } else {
                    println!("package.metadata.winres does not exist");
                }
            } else {
                println!("package.metadata does not exist");
            }
        } else {
            println!("package does not exist");
        }
    } else {
        println!("TOML parsing error")
    }
    Ok(())
}

pub(crate) fn escape_string(string: &str) -> String {
    let mut escaped = String::new();
    for chr in string.chars() {
        // In quoted RC strings, double-quotes are escaped by using two
        // consecutive double-quotes.  Other characters are escaped in the
        // usual C way using backslashes.
        match chr {
            '"' => escaped.push_str("\"\""),
            '\'' => escaped.push_str("\\'"),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(chr),
        };
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::escape_string;
    use super::get_sdk;

    #[test]
    fn string_escaping() {
        assert_eq!(&escape_string(""), "");
        assert_eq!(&escape_string("foo"), "foo");
        assert_eq!(&escape_string("\"Hello\""), "\"\"Hello\"\"");
        assert_eq!(&escape_string("C:\\Program Files\\Foobar"),
                   "C:\\\\Program Files\\\\Foobar");
    }

    #[cfg(target_env = "msvc")]
    #[test]
    fn test_get_sdk() {
        let tool = get_sdk().expect("get_sdk");
        println!("{:?}", tool);
    }
}
