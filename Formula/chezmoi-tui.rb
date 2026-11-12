class ChezmoiTui < Formula
  desc "Visual TUI wrapper around chezmoi"
  homepage "https://github.com/tetsuya-dev-jp/chezmoi-tui"
  url "https://github.com/tetsuya-dev-jp/chezmoi-tui.git", tag: "v0.3.1"
  license "MIT"
  head "https://github.com/tetsuya-dev-jp/chezmoi-tui.git", branch: "main"

  depends_on "chezmoi"
  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    actual = shell_output("#{bin}/chezmoi-tui --version").strip
    if build.head?
      assert_match(/\Achezmoi-tui \d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?\z/, actual)
    else
      assert_equal "chezmoi-tui #{version}", actual
    end
  end
end
