#!/usr/bin/env bash
#
# Print the Homebrew cask for Leaf.app — `Casks/leaf-editor.rb` in
# diaryx-org/homebrew-tap — for one release:
#
#   scripts/render-cask.sh <version> <sha256 of Leaf-<version>-aarch64.dmg>
#
# mac-app.yml runs this on a release tag and pushes the result to the tap; the
# tap is written by CI and by nothing else. It is a script rather than a
# heredoc in the workflow so that the cask can be rendered and `brew audit`ed
# on a Mac before a tag depends on it.
#
# `leaf-editor`, not `leaf`: the tap's `leaf` is the terminal editor's formula,
# and a cask of the same name would leave `brew install diaryx-org/tap/leaf`
# meaning the formula and the app needing `--cask` to be reached at all.
set -euo pipefail

if [ $# -ne 2 ]; then
  echo "usage: $0 <version> <sha256>" >&2
  exit 2
fi
version="$1"
sha256="$2"

cat <<RUBY
cask "leaf-editor" do
  version "$version"
  sha256 "$sha256"

  url "https://github.com/diaryx-org/leaf/releases/download/v#{version}/Leaf-#{version}-aarch64.dmg"
  name "Leaf"
  desc "Caret-based rich-text editor for Markdown, Djot, and HTML documents"
  homepage "https://github.com/diaryx-org/leaf"

  depends_on arch: :arm64
  depends_on macos: :ventura

  app "Leaf.app"

  zap trash: [
    "~/Library/Caches/org.diaryx.leaf",
    "~/Library/Preferences/org.diaryx.leaf.plist",
    "~/Library/Saved Application State/org.diaryx.leaf.savedState",
  ]
end
RUBY
