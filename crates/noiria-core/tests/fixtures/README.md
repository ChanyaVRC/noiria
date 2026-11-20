# VP9 HBD fixtures

The two IVF files below were synthesized from FFmpeg's `lavfi testsrc2` source. They do not
contain copied media or data from an external VP9 test corpus.

Generator: `ffmpeg version 8.1.2-full_build-www.gyan.dev`

Run the commands from the repository root.

## `vp9_p2_10bit_yuv420_16x16_2f.ivf`

- VP9 profile 2, 10-bit YUV 4:2:0, 16x16, two displayed frames
- SHA-256: `f1915ef7e850c2b8e0a9b4af0439aa34ad932f96b7251127e9f0daaf5154b6af`

```powershell
ffmpeg -hide_banner -loglevel error -f lavfi -i "testsrc2=size=16x16:rate=1:duration=2" -frames:v 2 -vf format=yuv420p10le -an -c:v libvpx-vp9 -profile:v 2 -pix_fmt yuv420p10le -lossless 1 -deadline best -cpu-used 0 -threads 1 -row-mt 0 -tile-columns 0 -tile-rows 0 -frame-parallel 0 -auto-alt-ref 0 -lag-in-frames 0 -g 999 -fflags +bitexact -flags:v +bitexact -f ivf -y "crates/noiria-core/tests/fixtures/vp9_p2_10bit_yuv420_16x16_2f.ivf"
```

## `vp9_p3_12bit_yuv444_16x16_2f.ivf`

- VP9 profile 3, 12-bit YUV 4:4:4, 16x16, two displayed frames
- SHA-256: `3c67156c4a4c88d36d15be8f1db928e7d04a9a66df0c31ca9118c796a8f36805`

```powershell
ffmpeg -hide_banner -loglevel error -f lavfi -i "testsrc2=size=16x16:rate=1:duration=2" -frames:v 2 -vf format=yuv444p12le -an -c:v libvpx-vp9 -profile:v 3 -pix_fmt yuv444p12le -lossless 1 -deadline best -cpu-used 0 -threads 1 -row-mt 0 -tile-columns 0 -tile-rows 0 -frame-parallel 0 -auto-alt-ref 0 -lag-in-frames 0 -g 999 -fflags +bitexact -flags:v +bitexact -f ivf -y "crates/noiria-core/tests/fixtures/vp9_p3_12bit_yuv444_16x16_2f.ivf"
```
