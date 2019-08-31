use std::path::{Path, PathBuf};
use std::fmt;
use std::collections::HashMap;
use std::io;
use std::process;
use std::error::Error;

pub const INSTALLED_ROOTS_KEY: &'static str = r"HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows Kits\Installed Roots";

#[derive(PartialEq,Eq,Debug,Clone,Hash)]
pub struct KitsRoot(String);

#[derive(PartialEq,Eq,Debug,Copy,Clone,Hash)]
pub enum Arch {
    Arm,
    Arm64,
    X64,
    X86,
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.dirname())
    }
}

impl Arch {
    pub fn arch_for_cfg_target() -> Option<Arch> {
        if cfg!(target_arch = "x86_64") {
            Some(Arch::X64)
        } else if cfg!(target_arch = "x86") {
            Some(Arch::X86)
        } else {
            None
        }
    }

    pub fn dirname(&self) -> &'static str {
        match self  {
            Arch::Arm => "arm",
            Arch::Arm64 => "arm64",
            Arch::X64 => "x64",
            Arch::X86 => "x86",
        }
    }
}

#[derive(Debug,Clone)]
pub struct SdkArch {
    pub bin_dir: PathBuf,
    pub include_dirs: HashMap<String, PathBuf>,
    pub lib_dirs: HashMap<String, PathBuf>,
}

impl SdkArch {
    pub fn new(bin_dir: PathBuf) -> Self {
        SdkArch {
            bin_dir: bin_dir,
            include_dirs: HashMap::new(),
            lib_dirs: HashMap::new(),
        }
    }
}

#[derive(Debug,Clone)]
pub struct Sdk {
    pub version: String,
    pub installed_root: PathBuf,
    pub sdk_archs: HashMap<Arch,SdkArch>,
}

pub struct ToolRef {
    pub arch: Arch,
    pub name: String,
}

#[derive(Debug,Clone,PartialEq)]
pub struct Tool {
    pub sdk_version: String,
    pub installed_root: PathBuf,
    pub arch: Arch,
    pub path: PathBuf,
    pub include_dirs: HashMap<String,PathBuf>,
    pub lib_dirs: HashMap<String,PathBuf>,
    pub bin_dir: PathBuf,
}

impl Sdk {
    pub fn new(version: String, installed_root: PathBuf) -> io::Result<Sdk> {
        let mut sdk = Sdk {
            version: version,
            installed_root: installed_root,
            sdk_archs: HashMap::new(),
        };
        sdk.load_archs()?;
        Ok(sdk)
    }

    pub fn tool(&self, name: &str, arch: Arch) -> Option<Tool> {
        if let Some(sdk_arch) = self.sdk_archs.get(&arch) {
            let path = sdk_arch.bin_dir.join(name);
            if path.exists() {
                Some(Tool {
                    sdk_version: self.version.clone(),
                    installed_root: self.installed_root.clone(),
                    arch: arch,
                    path: path,
                    include_dirs: sdk_arch.include_dirs.clone(),
                    lib_dirs: sdk_arch.lib_dirs.clone(),
                    bin_dir: sdk_arch.bin_dir.clone(),
                })
            } else { 
                None
            }
        } else {
            None
        }
    }

    pub fn exists(version: &str, installed_root: &Path) -> io::Result<bool> {
        Ok(installed_root.join("bin").join(version).exists())
    }

    pub fn bin_root_dir(&self) -> PathBuf {
        self.installed_root.join("bin").join(&self.version)
    }

    pub fn lib_root_dir(&self) -> PathBuf {
        self.installed_root.join("Lib").join(&self.version)
    }

    pub fn include_root_dir(&self) -> PathBuf {
        self.installed_root.join("Include").join(&self.version)
    }

    fn load_include_dirs(&self) -> io::Result<HashMap<String,PathBuf>> {
        let mut dirs = HashMap::new();
        for include_dir in self.include_root_dir().read_dir()? {
            let entry = include_dir?;
            dirs.insert(entry.file_name().to_string_lossy().to_owned().into_owned(), entry.path());
        }
        Ok(dirs)
    }

    pub fn sdk_arch<'a>(&'a self, arch: &Arch) -> Option<&'a SdkArch> {
        self.sdk_archs.get(arch)
    }

    pub fn has_tool(&self, arch: &Arch, tool: &str) -> bool {
        self.sdk_arch(arch)
            .map_or(false, |sa| {
                sa.bin_dir.join(tool).exists()
            })
    }

    fn load_archs(&mut self) -> io::Result<()> {
        self.load_arch(Arch::Arm)?;
        self.load_arch(Arch::Arm64)?;
        self.load_arch(Arch::X86)?;
        self.load_arch(Arch::X64)?;
        Ok(())
    }

    fn load_arch(&mut self, arch: Arch) -> io::Result<()> {
        let bin_dir = self.bin_root_dir().join(arch.dirname());
        let mut sdk_arch = SdkArch::new(bin_dir);
        sdk_arch.include_dirs = self.load_include_dirs()?;
        self.sdk_archs.insert(arch, sdk_arch);
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct System {
    pub installed_roots: InstalledRoots,
    pub sdks: Vec<Sdk>,
}

impl System {
    pub fn new() -> io::Result<Self> {
        let mut system = System {
            installed_roots: InstalledRoots::new()?,
            sdks: vec![],
        };
        system.load_sdks()?;
        Ok(system)
    }

    fn load_sdks(&mut self) -> io::Result<()> {
        for (_kits_root, root_path) in self.installed_roots.kits_roots.iter() {
            for sdk_version in self.installed_roots.sdk_versions.iter() {
                if Sdk::exists(&sdk_version, &root_path)? {
                    self.sdks.push(Sdk::new(sdk_version.clone(), root_path.clone())?);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug,PartialEq,Clone)]
pub struct InstalledRoots {
    pub kits_roots: Vec<(KitsRoot, PathBuf)>,
    pub sdk_versions: Vec<String>,
}

impl InstalledRoots {
    pub fn new() -> io::Result<InstalledRoots> {
        let output = process::Command::new("reg")
            .arg("query")
            .arg(INSTALLED_ROOTS_KEY)
            .arg("/reg:32")
            .output()?;

        let lines = String::from_utf8(output.stdout)
            .or_else(|e| Err(io::Error::new(io::ErrorKind::Other, e.description())))?;

        let mut roots = vec![];
        let mut sdks = vec![];

        for line in lines.lines() {
            let line = line.trim();
            if line.starts_with("KitsRoot") {
                let kits_root = KitsRoot(line.chars().take_while(|c| !c.is_whitespace()).collect());

                let root = Path::new(
                        &line.chars()
                        .skip(line.find("REG_SZ").ok_or_else(||
                            io::Error::new(io::ErrorKind::Other, "missing REG_SZ"))
                            .expect("parse line") + 6)
                        .skip_while(|c| c.is_whitespace())
                        .collect::<String>()
                    )
                    .to_path_buf();
                roots.push((kits_root, root));
            } else if line.starts_with(INSTALLED_ROOTS_KEY) {
                let sdk_version = line.chars().skip(INSTALLED_ROOTS_KEY.len() + 1).collect::<String>();
                if !sdk_version.is_empty() {
                    sdks.push(sdk_version);
                }
            }
        };

        if !roots.is_empty() {
            Ok(InstalledRoots {
                kits_roots: roots,
                sdk_versions: sdks,
            })
        } else {
            Err(io::Error::new(io::ErrorKind::Other, format!("no installed root found")))
        }
    }
}

/// Find a Windows SDK
pub fn get_sdk() -> io::Result<Vec<PathBuf>> {
    let mut kits: Vec<PathBuf> = Vec::new();
    let roots = InstalledRoots::new()?;

    for (_kits_root, root_path) in roots.kits_roots {
        let rc = if cfg!(target_arch = "x86_64") {
            root_path.join(r"bin\x64\rc.exe")
        } else {
            root_path.join(r"bin\x86\rc.exe")
        };

        if rc.exists() {
            println!("{:?}", rc);
            kits.push(rc.parent().unwrap().to_owned());
        }

        for sdk_version in roots.sdk_versions.iter() {
            let sdk_path = root_path.join("bin").join(sdk_version);
            let p = if cfg!(target_arch = "x86_64") {
                sdk_path.join(r"x64\rc.exe")
            } else {
                sdk_path.join(r"x86\rc.exe")
            };
            
            if p.exists() {
                println!("{:?}", p);
                kits.push(p.parent().unwrap().to_owned());
            }
        }
    }

    Ok(kits)
}

#[cfg(test)]
mod tests {
    use super::{get_sdk, InstalledRoots, System};

    #[cfg(target_env = "msvc")]
    #[test]
    #[ignore]
    fn test_get_sdk() {
        let sdks = get_sdk().expect("get_sdk");
        assert!(!sdks.is_empty());
        println!("{:?}", sdks);
        assert!(sdks.get(0).is_some());
    }

    #[cfg(target_env = "msvc")]
    #[test]
    fn test_get_installed_roots() {
        let roots = InstalledRoots::new().expect("get_installed_roots");
        println!("{:?}", roots);
    }

    #[cfg(target_env = "msvc")]
    #[test]
    fn test_system() {
        let system = System::new().expect("system::new");
        println!("{:?}", system);
    }
}
