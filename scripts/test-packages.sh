#!/usr/bin/env bash
set -e

echo "=== TermIDE Package Build Test ==="
echo "Testing package builds before release..."
echo ""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

TARGET="x86_64-unknown-linux-gnu"

# Check if cargo-deb is installed
if ! command -v cargo-deb &> /dev/null; then
    echo -e "${YELLOW}Installing cargo-deb...${NC}"
    cargo install cargo-deb
fi

# Check if cargo-generate-rpm is installed
if ! command -v cargo-generate-rpm &> /dev/null; then
    echo -e "${YELLOW}Installing cargo-generate-rpm...${NC}"
    cargo install cargo-generate-rpm
fi

echo ""
echo "=== Testing .deb package build ==="
echo "Building for target: $TARGET"
if cargo deb --target "$TARGET"; then
    DEB_FILE=$(find target/$TARGET/debian -name "*.deb" | head -n 1)
    if [ -n "$DEB_FILE" ]; then
        echo -e "${GREEN}✓ .deb package built successfully: $DEB_FILE${NC}"
        ls -lh "$DEB_FILE"
    else
        echo -e "${RED}✗ .deb file not found in target/$TARGET/debian/${NC}"
        exit 1
    fi
else
    echo -e "${RED}✗ .deb build failed${NC}"
    exit 1
fi

echo ""
echo "=== Testing .rpm package build ==="
echo "Building for target: $TARGET"
if cargo build --release --target "$TARGET"; then
    echo -e "${GREEN}✓ Binary built successfully${NC}"
    if cargo generate-rpm --target "$TARGET"; then
        RPM_FILE=$(find target/$TARGET/generate-rpm -name "*.rpm" | head -n 1)
        if [ -n "$RPM_FILE" ]; then
            echo -e "${GREEN}✓ .rpm package built successfully: $RPM_FILE${NC}"
            ls -lh "$RPM_FILE"
        else
            echo -e "${RED}✗ .rpm file not found in target/$TARGET/generate-rpm/${NC}"
            exit 1
        fi
    else
        echo -e "${RED}✗ .rpm generation failed${NC}"
        exit 1
    fi
else
    echo -e "${RED}✗ Binary build failed${NC}"
    exit 1
fi

echo ""
echo -e "${GREEN}=== All package builds passed! ===${NC}"
echo "You can now proceed with the release."
