#!/bin/bash
set -e

# Configuration
MAIN_REPO="$(cd "$(dirname "$0")" && pwd)"
FLATHUB_REPO="/home/danst/Skripte/me.dumke.Reinschrift"
MANIFEST="me.dumke.Reinschrift.yml"

# Get version from Cargo.toml
VERSION=$(grep -m1 '^version = ' "$MAIN_REPO/Cargo.toml" | sed 's/version = "\(.*\)"/\1/')
TAG="v$VERSION"

echo "=== Flathub Publisher ==="
echo "Version: $VERSION"
echo "Tag: $TAG"
echo ""

# Step 1: Ensure we're on main and up to date
echo "[1/7] Checking main repo status..."
cd "$MAIN_REPO"
if [[ -n $(git status --porcelain) ]]; then
    echo "Error: Main repo has uncommitted changes. Commit or stash them first."
    exit 1
fi

# Step 2: Create and push tag if it doesn't exist
echo "[2/7] Creating git tag..."
if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "Tag $TAG already exists, skipping..."
else
    git tag "$TAG"
    echo "Created tag $TAG"
fi

echo "[3/7] Pushing tag to origin..."
git push origin "$TAG" 2>/dev/null || echo "Tag already pushed or no changes"

# Step 3: Update Flathub repo
echo "[4/7] Updating Flathub repo..."
cd "$FLATHUB_REPO"
git fetch origin

# Check if branch already exists
if git rev-parse --verify "origin/$VERSION" >/dev/null 2>&1; then
    echo "Branch $VERSION already exists on remote. Checking out..."
    git checkout "$VERSION"
    git pull origin "$VERSION"
else
    # Create new branch from master
    git checkout master
    git pull origin master
    git checkout -b "$VERSION"
fi

# Step 4: Update manifest
echo "[5/7] Updating manifest..."
sed -i "s/tag: v[0-9.]*$/tag: $TAG/" "$MANIFEST"

# Step 5: Regenerate cargo sources
echo "[6/7] Regenerating cargo sources..."
python3 "$MAIN_REPO/flatpak-cargo-generator.py" \
    "$MAIN_REPO/Cargo.lock" \
    -o generated-sources.json

# Step 6: Commit and push
echo "[7/7] Committing and pushing..."
git add "$MANIFEST" generated-sources.json

if git diff --cached --quiet; then
    echo "No changes to commit"
else
    git commit -m "Update to $TAG"
fi

git push -u origin "$VERSION"

# Step 7: Create PR using gh CLI
echo ""
echo "=== Creating Pull Request ==="
if command -v gh &> /dev/null; then
    # Check if PR already exists
    EXISTING_PR=$(gh pr list --head "$VERSION" --json number --jq '.[0].number' 2>/dev/null || echo "")
    if [[ -n "$EXISTING_PR" ]]; then
        echo "PR #$EXISTING_PR already exists:"
        echo "https://github.com/flathub/me.dumke.Reinschrift/pull/$EXISTING_PR"
    else
        gh pr create \
            --title "Update to $TAG" \
            --body "Updates Reinschrift to version $VERSION

Changes: https://github.com/danst0/ReinschriftTodo/releases/tag/$TAG" \
            --base master \
            --head "$VERSION"
    fi
else
    echo "gh CLI not installed. Create PR manually at:"
    echo "https://github.com/flathub/me.dumke.Reinschrift/compare/master...$VERSION"
fi

echo ""
echo "=== Done ==="
echo "Flathub will build and test the package automatically."
echo "Once the PR is merged, it will be published."
