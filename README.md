# Okuri

Dead-simple file transfer for Omarchy & Linux.

![Okuri's icon](./assets/icon.png)

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

https://github.com/user-attachments/assets/344c2f14-9de4-46a4-b971-f980992086d7

### Multiple windows

Connect to multiple remotes at once, and drag-and-drop files between them.

https://github.com/user-attachments/assets/ca734d76-817e-483a-a41e-118f1414f390

### Metadata

Check every detail about a file and modify it, all from one place.

<img width="720" height="599" alt="File view in Okuri" src="https://github.com/user-attachments/assets/827cf85c-2c48-4389-bab0-2153ecd81b17" />

### Omarchy themes

Okuri has full theming support for Omarchy. It will adapt it's looks to whatever theme is currently set and updates live whenever the theme changes.

Here are a few screenshots of Okuri in different Omarchy themes:

| Theme | |
|-|-|
| Lupine | <img width="520" height="430" alt="Okuri in Lupine theme" src="https://github.com/user-attachments/assets/b9e82da1-7ff3-4376-a1ac-ad5de5158614" /> |
| Rose Pine | <img width="520" height="430" alt="Okuri in Rose Pine theme" src="https://github.com/user-attachments/assets/c3be3aed-63e0-4990-871d-1de4655ed49a" /> |
| White | <img width="520" height="430" alt="Okuri in White theme" src="https://github.com/user-attachments/assets/ae2da2a5-8406-41f8-8e3b-8aa869624563" /> |
| Tokyo Night | <img width="520" height="430" alt="Okuri in Tokyo Night theme" src="https://github.com/user-attachments/assets/dd8058ff-64fc-4cba-a0dd-39de4f51634b" /> |
| Retro 82 | <img width="520" height="430" alt="Okuri in Retro 82 theme" src="https://github.com/user-attachments/assets/4064700e-0049-46b9-b637-4b0c8890ccf0" /> |
| Nord | <img width="520" height="430" alt="Okuri in Nord theme" src="https://github.com/user-attachments/assets/48b583c7-889a-465a-8cae-42c621d08974" /> |
| Kanagawa | <img width="520" height="430" alt="Okuri in Kanagawa theme" src="https://github.com/user-attachments/assets/b5361a35-d0e3-4189-842a-bc689f70a187" /> |
| Hackerman | <img width="520" height="430" alt="Okuri in Hackerman theme" src="https://github.com/user-attachments/assets/6d0a6356-7b3a-4cb7-b467-680f4b1536ac" /> |
| Gruvbox | <img width="520" height="430" alt="Okuri in Gruvbox theme" src="https://github.com/user-attachments/assets/76fa7889-9a5f-4d38-828b-dad33a4cc4be" /> |
| Catppuccin | <img width="520" height="430" alt="Okuri in Catppuccin theme" src="https://github.com/user-attachments/assets/a5e5cd71-1b87-4c05-8208-ee5347f192fd" /> |

## For nerds

Okuri is a native application built with Qt (with QML) and Rust.
Qt 6 is dynamically linked, so you have to have it installed on your machine for Okuri to run.

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
