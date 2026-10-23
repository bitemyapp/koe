set positional-arguments
set shell := ["bash", "scripts/just-shell", "-cu"]

# List the checkout's development commands.
default:
    @just --list

# Build the CPU package (or pass koe-vulkan for GPU support).
build package="koe":
    nix build ".#$1" --cores 4 --max-jobs 1

# Rebuild and install this checkout, preserving the installed CPU/Vulkan variant.
install package="auto":
    @bash scripts/install "$1"

# Rebuild, reinstall and restart Koe, like Aizome's just replace command.
alias replace := install
