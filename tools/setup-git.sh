#!/bin/sh
# Set this clone up for work that moves between two machines. Run it once in
# every clone: the Mac's and the Windows PC's. On Windows, run it in Git Bash.
#
# The settings live in the clone, not in the repository, which is why this
# script exists. Line endings are different: .gitattributes settles those for
# every clone by itself.
set -eu
cd "$(dirname "$0")/.."

# A pull replays this machine's commits on top of the other machine's, so the
# history stays one straight line instead of a merge at every change of desk.
git config pull.rebase true
# ... and it works with uncommitted changes in the tree.
git config rebase.autoStash true
# Branches deleted on GitHub disappear here too.
git config fetch.prune true
# A new branch is pushed to GitHub under its own name without being told.
git config push.autoSetupRemote true
# .gitattributes decides line endings; Git's own guessing is turned off.
git config core.autocrlf false
# Paths under core/target and the Android build tree are long.
git config core.longpaths true

echo "this clone is set up; branch $(git rev-parse --abbrev-ref HEAD), remote $(git remote get-url origin 2>/dev/null || echo none)"
