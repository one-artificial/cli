#!/bin/bash
set -euo pipefail

INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
COMPLETIONS_DIR_ZSH="${HOME}/.zsh/completions"
COMPLETIONS_DIR_BASH="${BASH_COMPLETION_USER_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/bash-completion/completions}"

echo "Building One (release mode)..."
cargo build --release

BINARY="./target/release/one"

if [ ! -f "$BINARY" ]; then
    echo "Error: Build failed — binary not found at $BINARY"
    exit 1
fi

echo "Installing to $INSTALL_DIR/one..."
if [ -w "$INSTALL_DIR" ]; then
    cp "$BINARY" "$INSTALL_DIR/one"
else
    sudo cp "$BINARY" "$INSTALL_DIR/one"
fi

# Generate shell completions
echo "Generating shell completions..."

# Zsh
mkdir -p "$COMPLETIONS_DIR_ZSH"
"$INSTALL_DIR/one" --completions zsh > "$COMPLETIONS_DIR_ZSH/_one" 2>/dev/null || true

# Bash
mkdir -p "$COMPLETIONS_DIR_BASH"
"$INSTALL_DIR/one" --completions bash > "$COMPLETIONS_DIR_BASH/one" 2>/dev/null || true

# Fish
FISH_DIR="${HOME}/.config/fish/completions"
if [ -d "${HOME}/.config/fish" ]; then
    mkdir -p "$FISH_DIR"
    "$INSTALL_DIR/one" --completions fish > "$FISH_DIR/one.fish" 2>/dev/null || true
fi

# Create config directory
mkdir -p "${HOME}/.one"

echo ""
echo "Installed successfully!"
echo ""
echo "  one --help          Show usage"
echo "  one                 Start with current directory"
echo "  one -p /path        Start with a project"
echo ""
echo "Set up your API key:"
echo "  export ANTHROPIC_API_KEY=sk-ant-..."
echo "  # or use: one, then /login anthropic"
echo ""

# Check if zsh completions need fpath update
if [ -n "${ZSH_VERSION:-}" ] || [ "$SHELL" = "/bin/zsh" ]; then
    if ! grep -q "$COMPLETIONS_DIR_ZSH" "${HOME}/.zshrc" 2>/dev/null; then
        echo "For zsh completions, add to ~/.zshrc:"
        echo "  fpath=(~/.zsh/completions \$fpath)"
        echo "  autoload -Uz compinit && compinit"
    fi
fi
