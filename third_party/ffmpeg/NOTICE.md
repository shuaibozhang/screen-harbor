# FFmpeg runtime notice

FrameFlow distributes an unmodified FFmpeg command-line runtime as a separate process. FrameFlow does not link its proprietary/Rust code to FFmpeg libraries.

The packaged runtime is the BtbN `win64-lgpl-shared-9.0` build from release `autobuild-2026-08-29-13-12`:

- Binary archive: https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-08-29-13-12/ffmpeg-n9.0.1-11-ge47273f4d9-win64-lgpl-shared-9.0.zip
- Archive SHA-256: `e452726c9282e9d8b640dd29f012db91bf6133b490b5490a37e8b0a4d3ec7ae9`
- FFmpeg version: `n9.0.1-11-ge47273f4d9-20260829`
- FFmpeg source revision: https://github.com/FFmpeg/FFmpeg/commit/e47273f4d9
- Corresponding FFmpeg source archive: https://github.com/FFmpeg/FFmpeg/archive/e47273f4d9.tar.gz
- Build recipes and dependency sources: https://github.com/BtbN/FFmpeg-Builds/tree/autobuild-2026-08-29-13-12
- Build tooling license: https://github.com/BtbN/FFmpeg-Builds/blob/autobuild-2026-08-29-13-12/LICENSE

The installer includes `ffmpeg-source-e47273f4d9.tar.gz` and `ffmpeg-build-recipes-autobuild-2026-08-29-13-12.tar.gz` beside the runtime, so the corresponding FFmpeg source and build recipes remain available offline after installation. Their pinned hashes and origin URLs are recorded in `manifest.json`.

The runtime reports GNU Lesser General Public License version 3 or later and was built with `--enable-shared --disable-static`; it does not include `--enable-gpl` or `--enable-nonfree`. The complete LGPL text supplied by the binary distributor is installed as `LICENSE.txt` beside `ffmpeg.exe`.

FFmpeg and the third-party libraries included by the upstream build remain the property of their respective copyright holders and are distributed under their respective licenses. This notice is not legal advice; release owners should complete their own jurisdiction-specific patent and license review before commercial distribution.
