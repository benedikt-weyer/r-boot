#! @bash@/bin/sh -e

shopt -s nullglob

export PATH=/empty:@path@

usage() {
    echo "usage: $0 -t <timeout> -c <path-to-default-configuration> -d <esp-mount-point> -b <r-boot-efi> [-g <num-generations>] [-e]" >&2
    exit 1
}

timeout=                # r-boot menu timeout, in seconds
default=                # Default (current) system configuration
esp=                    # EFI system partition mount point
binary=                 # r-boot EFI binary to install
numGenerations=0        # Number of older generations to include in the menu
touchEfiVars=           # Whether to register a boot entry via efibootmgr

while getopts "t:c:d:b:g:e" opt; do
    case "$opt" in
        t) timeout="$OPTARG" ;;
        c) default="$OPTARG" ;;
        d) esp="$OPTARG" ;;
        b) binary="$OPTARG" ;;
        g) numGenerations="$OPTARG" ;;
        e) touchEfiVars=1 ;;
        \?) usage ;;
    esac
done

[ -z "$timeout" -o -z "$default" -o -z "$esp" -o -z "$binary" ] && usage

target="$esp/boot"
mkdir -p "$target/nixos"

# Convert a path to a file in the Nix store such as
# /nix/store/<hash>-<name>/file to <hash>-<name>-<file>.
cleanName() {
    local path="$1"
    echo "$path" | sed 's|^/nix/store/||' | sed 's|/|-|g'
}

# Copy a file from the Nix store to $target/nixos, deduplicated by name, and
# print its path relative to the ESP root (the form r-boot's config expects).
declare -A filesCopied

copyToKernelsDir() {
    local src=$(readlink -f "$1")
    local name=$(cleanName "$src")
    local dst="$target/nixos/$name"
    if ! test -e "$dst"; then
        local dstTmp="$dst.tmp.$$"
        cp "$src" "$dstTmp"
        mv "$dstTmp" "$dst"
    fi
    filesCopied[$dst]=1
    result="/boot/nixos/$name"
}

# Emit a `[[entries]]` table for one generation.
addEntry() {
    local path=$(readlink -f "$1")
    local id="$2"

    if ! test -e "$path/kernel" -a -e "$path/initrd"; then
        return
    fi

    copyToKernelsDir "$path/kernel"; kernel=$result
    copyToKernelsDir "$path/initrd"; initrd=$result

    local nixosLabel="$(cat "$path/nixos-version" 2>/dev/null || echo unknown)"
    local extraParams="$(cat "$path/kernel-params" 2>/dev/null || true)"

    echo
    echo "[[entries]]"
    echo "id = \"$id\""
    echo "title = \"NixOS ($nixosLabel, $id)\""
    echo "kind = \"linux\""
    echo "linux = \"$kernel\""
    echo "initrd = \"$initrd\""
    echo "options = \"init=$path/init $extraParams\""
}

tmpFile="$target/r-boot.toml.tmp.$$"

cat > "$tmpFile" <<EOF
# Generated file, all changes will be lost on nixos-rebuild!
default = "nixos-default"
timeout = $timeout
EOF

addEntry "$default" "nixos-default" >> "$tmpFile"

if [ "$numGenerations" -gt 0 ]; then
    # Add up to $numGenerations older generations, most recent first.
    for generation in $(
            (cd /nix/var/nix/profiles && ls -d system-*-link) \
            | sed 's/system-\([0-9]\+\)-link/\1/' \
            | sort -n -r \
            | head -n "$numGenerations"); do
        link=/nix/var/nix/profiles/system-$generation-link
        addEntry "$link" "nixos-generation-$generation"
    done >> "$tmpFile"
fi

mv -f "$tmpFile" "$target/r-boot.toml"

# Remove kernels/initrds that no longer belong to any kept generation.
for fn in "$target"/nixos/*; do
    if ! test "${filesCopied[$fn]}" = 1; then
        echo "Removing no longer needed boot file: $fn"
        chmod +w -- "$fn"
        rm -f -- "$fn"
    fi
done

install -Dm755 "$binary" "$esp/EFI/BOOT/BOOTX64.EFI"

if [ -n "$touchEfiVars" ] && command -v efibootmgr > /dev/null; then
    if ! efibootmgr | grep -q "r-boot"; then
        part=$(findmnt -n -o SOURCE --target "$esp")
        partName=$(basename "$part")
        diskName=$(lsblk -no pkname "$part" 2>/dev/null | head -n1)
        partNum=$(cat "/sys/class/block/$partName/partition" 2>/dev/null)
        if [ -n "$diskName" -a -n "$partNum" ]; then
            efibootmgr --create --disk "/dev/$diskName" --part "$partNum" \
                --label "r-boot" --loader '\EFI\BOOT\BOOTX64.EFI' > /dev/null || true
        fi
    fi
fi
