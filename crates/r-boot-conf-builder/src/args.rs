use std::path::PathBuf;

pub struct Args {
    /// r-boot menu timeout, in seconds.
    pub timeout: u32,
    /// Path to the default (current) system configuration.
    pub default: PathBuf,
    /// EFI system partition mount point.
    pub esp: PathBuf,
    /// r-boot EFI binary to install.
    pub binary: PathBuf,
    /// Number of older generations to include in the menu.
    pub num_generations: u32,
    /// Whether to register a boot entry via efibootmgr.
    pub touch_efi_vars: bool,
}

impl Args {
    pub fn usage() {
        eprintln!(
            "usage: r-boot-conf-builder -t <timeout> -c <path-to-default-configuration> \
             -d <esp-mount-point> -b <r-boot-efi> [-g <num-generations>] [-e]"
        );
    }

    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut timeout = None;
        let mut default = None;
        let mut esp = None;
        let mut binary = None;
        let mut num_generations = 0;
        let mut touch_efi_vars = false;

        while let Some(arg) = args.next() {
            let mut value = || {
                args.next()
                    .ok_or_else(|| format!("{arg}: missing argument"))
            };
            match arg.as_str() {
                "-t" => timeout = Some(value()?.parse::<u32>().map_err(|e| e.to_string())?),
                "-c" => default = Some(PathBuf::from(value()?)),
                "-d" => esp = Some(PathBuf::from(value()?)),
                "-b" => binary = Some(PathBuf::from(value()?)),
                "-g" => num_generations = value()?.parse::<u32>().map_err(|e| e.to_string())?,
                "-e" => touch_efi_vars = true,
                other => return Err(format!("unrecognized argument: {other}")),
            }
        }

        Ok(Args {
            timeout: timeout.ok_or("-t <timeout> is required")?,
            default: default.ok_or("-c <path-to-default-configuration> is required")?,
            esp: esp.ok_or("-d <esp-mount-point> is required")?,
            binary: binary.ok_or("-b <r-boot-efi> is required")?,
            num_generations,
            touch_efi_vars,
        })
    }
}
