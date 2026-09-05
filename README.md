# Okuri

Dead-simple file transfer for Omarchy / Linux.

<img src="./assets/icon.png" alt="Okuri's icon" width="256">

Supports:
- SFTP
- FTP/FTPS
- Amazon S3 (and compatibles)
- Cloudflare R2
- Backblaze B2
- Azure Blob Storage
- WebDAV

## Install

**Omarchy:**
```bash
omarchy pkg aur add okuri-bin
# omarchy pkg aur add okuri # builds from source
```

**Arch:**
```bash
yay -S okuri-bin   # prebuilt, no compile
# yay -S okuri       # builds from source
```

**Anything else:**
```bash
curl -fsSL https://raw.githubusercontent.com/monorkin/okuri/main/install.sh | bash
```

## Features

### Drag and drop

Upload files by drag-and-dropping them into Okuri.

https://github.com/user-attachments/assets/0d8ff82f-a97d-4fd2-875a-21ee834a1038

### Multiple windows

Connect to multiple remotes at once, and drag-and-drop files between them.

https://github.com/user-attachments/assets/fe8f18f0-9fa4-4ea8-a33e-0c8776d0f59b

### Metadata

Check every detail about a file and modify it, all from one place.

| Context | Screenshot |
|-|-|
| File actions | <img width="1261" height="1048" alt="okuri-file-actions" src="https://github.com/user-attachments/assets/9f94409c-4f4e-4714-9f14-b5a2e3f03777" /> |
| FTP file options | <img width="1261" height="1048" alt="okuri-ftp-file-options" src="https://github.com/user-attachments/assets/0d31c371-cc68-49a5-9518-e0e5718337e1" /> |
| S3 file options | <img width="1261" height="1048" alt="okuri-s3-file-options" src="https://github.com/user-attachments/assets/4d009e2e-6c05-4455-9a31-c03e4bab6208" /> |

### Omarchy themes

Okuri has full theming support for Omarchy. It adapts its looks to whatever theme is currently set and updates live whenever the theme changes.

Here are a few screenshots of Okuri in different Omarchy themes:

| Theme | Files | Menu |
|-|-|-|
| Lupine | <img width="1261" height="1048" alt="okuri-omarchy-theme-lupine-files" src="https://github.com/user-attachments/assets/e80e7a5f-07ba-43dc-89c8-63f19f80035f" /> | <img width="1266" height="1048" alt="okuri-omarchy-theme-lupine-menu" src="https://github.com/user-attachments/assets/031cccfa-b921-4c49-9a09-6d05e89790d1" /> |
| Rose Pine | <img width="1261" height="1048" alt="okuri-omarchy-theme-rose-pine-files" src="https://github.com/user-attachments/assets/f1379bc3-8dda-4fcc-a0a5-c4d70d33b9fe" /> | <img width="1266" height="1048" alt="okuri-omarchy-theme-rose-pine-menu" src="https://github.com/user-attachments/assets/4979ff3d-ab60-494a-8d2a-d23d4ce7cbb4" /> |
| Catppuccin Latte | <img width="1261" height="1048" alt="okuri-omarchy-theme-catppuccin-latte-files" src="https://github.com/user-attachments/assets/722abfc4-ec94-4a67-bb3b-0828189e063b" /> | <img width="1261" height="1048" alt="okuri-omarchy-theme-catppuccin-latte-menu" src="https://github.com/user-attachments/assets/cc829352-ea78-4ceb-b033-93f4295a1b52" /> |
| Catppuccin | <img width="1261" height="1048" alt="okuri-omarchy-theme-catppuccin-files" src="https://github.com/user-attachments/assets/a382a680-aa0d-4922-8f9a-902208fbe34e" /> | <img width="1261" height="1048" alt="okuri-omarchy-theme-catppuccin-menu" src="https://github.com/user-attachments/assets/b7a0e0c7-bc0f-40bc-bdc1-7ec205bfacdb" /> |
| Tokyo Night | <img width="1261" height="1048" alt="okuri-omarchy-theme-tokyo-night-files" src="https://github.com/user-attachments/assets/f7e9ffe8-fdad-4ab0-897e-6856a825d266" /> | <img width="1261" height="1048" alt="okuri-omarchy-theme-tokyo-night-menu" src="https://github.com/user-attachments/assets/f37cfebb-3b0a-4649-93f0-62dcac040948" /> |
| Retro 82 | <img width="1261" height="1048" alt="okuri-omarchy-theme-retro-82-files" src="https://github.com/user-attachments/assets/66e0397c-80f8-40ee-a76c-4cc25ccd7f48" /> | <img width="1261" height="1048" alt="okuri-omarchy-theme-retro-82-menu" src="https://github.com/user-attachments/assets/3c717853-3716-4e6f-9488-f4bd28af32a9" /> |
| Nord | <img width="1261" height="1048" alt="okuri-omarchy-theme-nord-files" src="https://github.com/user-attachments/assets/f9798e8a-0e9a-43d2-8c4d-cddf58daa2c6" /> | <img width="1261" height="1048" alt="okuri-omarchy-theme-nord-menu" src="https://github.com/user-attachments/assets/860579f0-3945-4278-aa8b-89db5e47eaec" /> |
| Everforest | <img width="1261" height="1048" alt="okuri-omarchy-theme-everforest-files" src="https://github.com/user-attachments/assets/d302a0cd-ce21-401f-9bcd-c382799b681f" /> | <img width="1261" height="1048" alt="okuri-omarchy-theme-everforest-menu" src="https://github.com/user-attachments/assets/167f08a6-f14a-4c66-99f9-8decd272d079" /> |
| Hackerman | <img width="1261" height="1048" alt="okuri-omarchy-theme-hackerman-files" src="https://github.com/user-attachments/assets/7673c1e8-2768-4b74-82bf-6e304e853f4f" /> | <img width="1261" height="1048" alt="okuri-omarchy-theme-hackerman-menu" src="https://github.com/user-attachments/assets/14ddb4f5-e6ae-45dd-b37e-21f75d332460" /> |
| Last Horizon | <img width="1261" height="1048" alt="okuri-omarchy-theme-last-horizon-files" src="https://github.com/user-attachments/assets/6dcbebea-c32f-457a-91ff-f0ab2a590e89" /> | <img width="1261" height="1048" alt="okuri-omarchy-theme-last-horizon-menu" src="https://github.com/user-attachments/assets/f027b71f-020b-42c1-8126-05b3a5982d80" /> |

## For nerds

Okuri is a native application built with GTK 4, libadwaita and Rust.
GTK 4 and libadwaita are dynamically linked, so both have to be installed on your machine for Okuri to run.

On Omarchy the window is painted from the current theme's `colors.toml` and follows a theme switch live. Anywhere else it falls back to a built-in dark or light palette, whichever the desktop asks for.

To run it locally:
```bash
cargo run -p okuri
```

To build it:
```bash
cargo build --release -p okuri
# this puts the built app in target/release/okuri
```

## License

Okuri is released under the MIT license. For more details read [LICENSE](./LICENSE)
