<p align="center">
  <img src="docs/icon.png" width="128" alt="ByteBroom icon">
</p>

<h1 align="center">ByteBroom</h1>

<p align="center">
  Finds what is safe to delete on your Mac and frees the space.<br>
  Build folders, caches, logs, simulators, leftover app files: sorted by size, with a reason for every item.
</p>

<p align="center">
  <a href="https://github.com/p32929/bytebroom/releases/latest"><b>Download for macOS</b></a>
</p>

---

Most disk cleaners guess by folder name and can wipe something you needed, or they just show a pie chart and leave the deciding to you. ByteBroom only offers what it can prove is safe, says why next to every item, and never touches a folder git tracks. It is free, open source and runs fully on your Mac.

## Screenshots

| | Light | Dark |
|---|---|---|
| **Clean up** | ![Clean up view, light mode](docs/clean-light.png) | ![Clean up view, dark mode](docs/clean-dark.png) |
| **Apps** | ![Apps view, light mode](docs/apps-light.png) | ![Apps view, dark mode](docs/apps-dark.png) |

ByteBroom follows your macOS appearance. You can switch theme in the bottom-right corner, next to the version number, which opens the About window.

## What it does

ByteBroom scans your whole disk, then puts everything over 50 MB that it knows how to judge into groups. Each item says why it is there, so you decide with the facts in front of you.

**Safe to delete:** these come back by themselves, or with a rebuild or reinstall.

1. **Project build folders:** `node_modules`, `target`, `build`, `.dart_tool`, `Pods`, `.venv` and more, for JavaScript, Rust, Flutter, iOS, Android, Python, Go, Swift, .NET, PHP, Elixir, C/C++, Zig, Haskell and Godot projects.
2. **Package manager caches:** npm, Yarn, pnpm, Bun, Cargo, Gradle, pip, uv, Go modules, Dart pub and others.
3. **Xcode build data:** DerivedData and module caches.
4. **App caches and logs.**

**Look first:** you lose what is inside, so check before deleting.

1. Trash, iOS simulators, Android emulators, Docker and VM data, Downloads, temporary files, backups and archives, and caches that sometimes hold logins or downloaded models.

**Apps:** every installed app, with the files it has spread around your system (Application Support, caches, preferences, containers, launch agents and more). Tick the box next to an app to remove it completely.

## Built to not delete the wrong thing

1. **A folder git tracks is never offered.** A `build` folder with committed files in it is source, not junk. Ambiguous names such as `dist` or `bin` are only offered when your `.gitignore` says they are generated.
2. **Projects need proof.** A `target` folder only counts if a `Cargo.toml` sits next to it; `node_modules` needs a `package.json`, and so on.
3. **Top-level folders are off limits.** ByteBroom refuses to remove your home folder, anything above it, or a system folder such as `/Library/Caches` itself.
4. **Links are removed, never followed.** Deleting a symlink leaves what it points to.
5. **You choose how:** *Move to Trash* can be undone; *Delete permanently* frees the space right away. A progress window shows each item as it goes.

## Install

1. Download the `.dmg` from the [latest release](https://github.com/p32929/bytebroom/releases/latest).
2. Open it and drag **ByteBroom** onto **Applications**.
3. Open ByteBroom from Launchpad, Spotlight or the Applications folder.

**"ByteBroom cannot be opened" the first time?** The app isn't notarized by Apple yet, so macOS asks you to allow it once:
open **System Settings → Privacy & Security**, scroll down, and click **Open Anyway** next to ByteBroom.
Or run this in Terminal:

```sh
xattr -dr com.apple.quarantine /Applications/ByteBroom.app
```

**Let it see everything:** add ByteBroom under **System Settings → Privacy & Security → Full Disk Access**. Without this, macOS hides some folders and ByteBroom finds less. If it is missing, ByteBroom explains this once on launch, before scanning, and opens the right settings pane for you; otherwise macOS would ask folder by folder during the scan.

Releases are built for Apple Silicon Macs. On an Intel Mac, build the Intel release yourself (see below) or build from source.

## Build from source

You need [Rust](https://rustup.rs).

```sh
git clone https://github.com/p32929/bytebroom.git
cd bytebroom
./run.sh                        # build and open the app
```

To install it into `/Applications` as a normal app:

```sh
cargo install cargo-bundle --locked  # once
./install.sh
```

To remove it again, with its caches, Desktop shortcut, privacy permissions and build output (your clone stays):

```sh
./uninstall.sh
```

`./release.sh` builds the `.dmg` and `.zip` that go on a GitHub release, into `dist/`, for the chip of the Mac you run it on. To build for the other chip, pass it: `./release.sh x86_64` (Intel) or `./release.sh arm64` (Apple Silicon). Cross-building needs the Rust target once: `rustup target add x86_64-apple-darwin` or `rustup target add aarch64-apple-darwin`. Use `cargo install cargo-bundle --locked`; without `--locked` it can fail to compile.

## Options

| Option | What it does |
|---|---|
| `--print` | Scan and print the list in the terminal, no window |
| `--apps` | Open on the Apps view |
| `--show=<name>` | Open on one group or app, e.g. `--show="Logs"` |
| `--light`, `--dark` | Start in that theme instead of following macOS |

Pass them after `./run.sh`, for example `./run.sh --print`.

## Requirements

macOS only. ByteBroom relies on macOS folders, the macOS Trash and app bundles, so it refuses to build anywhere else.

## Contributing

Contributions are very welcome, thank you! Before you start on a pull request, please open an issue and tell me what you would like to add or change. That way we can agree on it first, and your time is not wasted on something that might not fit. Thanks for helping make ByteBroom better.

## You might also like

<p><a href="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/0"><picture><source media="(prefers-color-scheme: dark)" srcset="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/0-dark.svg"><img src="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/0-light.svg" width="50%" align="top" alt="More apps by p32929 on OpenStore (1)"></picture></a><a href="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/1"><picture><source media="(prefers-color-scheme: dark)" srcset="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/1-dark.svg"><img src="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/1-light.svg" width="50%" align="top" alt="More apps by p32929 on OpenStore (2)"></picture></a><br><a href="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/2"><picture><source media="(prefers-color-scheme: dark)" srcset="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/2-dark.svg"><img src="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/2-light.svg" width="50%" align="top" alt="More apps by p32929 on OpenStore (3)"></picture></a><a href="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/3"><picture><source media="(prefers-color-scheme: dark)" srcset="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/3-dark.svg"><img src="https://openstore.pfay.workers.dev/more/p32929/bytebroom/personal/3-light.svg" width="50%" align="top" alt="More apps by p32929 on OpenStore (4)"></picture></a></p>

<p><a href="https://p32929.github.io/share/?repo=bytebroom"><picture><source media="(prefers-color-scheme: dark)" srcset="https://p32929.github.io/readme/footer-share-dark.svg"><img src="https://p32929.github.io/readme/footer-share-light.svg" width="50%" align="top" alt="Share this project with your friends in one click"></picture></a><a href="https://p32929.github.io/support/"><picture><source media="(prefers-color-scheme: dark)" srcset="https://p32929.github.io/readme/footer-support-dark.svg"><img src="https://p32929.github.io/readme/footer-support-light.svg" width="50%" align="top" alt="If you like my work and want to support me, feel free to support"></picture></a><br><a href="https://openstore.pfay.workers.dev/add"><picture><source media="(prefers-color-scheme: dark)" srcset="https://p32929.github.io/readme/footer-join-dark.svg"><img src="https://p32929.github.io/readme/footer-join-light.svg" width="100%" align="top" alt="Put your own apps on OpenStore: free, your listing lives in your own repo"></picture></a></p>
