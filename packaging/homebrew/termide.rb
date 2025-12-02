class Termide < Formula
  desc "Cross-platform terminal IDE, file manager and virtual terminal"
  homepage "https://github.com/termide/termide"
  version "0.1.4"
  license "MIT"

  on_macos do
    on_intel do
      url "https://github.com/termide/termide/releases/download/0.1.4/termide-0.1.4-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_ACTUAL_SHA256_x86_64_darwin"  # Update after release
    end

    on_arm do
      url "https://github.com/termide/termide/releases/download/0.1.4/termide-0.1.4-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_ACTUAL_SHA256_aarch64_darwin"  # Update after release
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/termide/termide/releases/download/0.1.4/termide-0.1.4-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_ACTUAL_SHA256_x86_64_linux"  # Update after release
    end

    on_arm do
      url "https://github.com/termide/termide/releases/download/0.1.4/termide-0.1.4-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_ACTUAL_SHA256_aarch64_linux"  # Update after release
    end
  end

  def install
    bin.install "termide"

    # Install help files
    (share/"termide/help").install "help/en.txt"
    (share/"termide/help").install "help/ru.txt"

    # Install documentation
    doc.install "README.md"
    doc.install "LICENSE"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/termide --version")
  end
end
