# quarry

A small voxel sandbox in a window, in one Zinkite file of about 1,800 lines. It draws
with OpenGL through SDL2, both reached through their C headers.

- **The world:** 256 × 256 × 80 blocks, generated from gradient noise at start. It has:
  - rolling land and forests;
  - beaches and a shallow sea;
  - rocky, snow-capped peaks;
  - worm caves;
  - coal and zincite ores;
  - trees, flowers and tall grass.
- **Moving and building:** walk, jump, swim and fly, colliding with the blocks. Break and
  place blocks, from a hotbar of nine.
- **What you see:**
  - textures painted by the program at start, so it needs no image files;
  - ambient occlusion at block corners, and sky light that dims under cover;
  - fog and see-through water;
  - drifting clouds and the open sea beyond the world's edge.
- **Saving:** worlds are saved to a file and read back. Screenshots are written as PPM
  images.

## Running it

You need SDL2's and OpenGL's development files, and clang, which reads their headers:

| System | |
|---|---|
| Debian, Ubuntu | `sudo apt install libsdl2-dev libgl-dev clang` |
| Fedora | `sudo dnf install SDL2-devel mesa-libGL-devel clang` |
| Arch | `sudo pacman -S sdl2 mesa clang` |
| NixOS | `nix-shell -p SDL2 libGL`, then the commands below in that shell |

```
zink run -O examples/quarry/quarry.zn                 # a new world from seed 1
zink run -O examples/quarry/quarry.zn --seed 42
zink run -O examples/quarry/quarry.zn --world my.qry  # read my.qry if it exists, save it on quit
zink build examples/quarry/quarry.zn -o quarry        # an executable
```

It starts in about 0.3 s at `-O`, and in about a second without it. Generating the world
and building its 256 chunk meshes are most of that.

| Keys | |
|---|---|
| mouse | look |
| W A S D | walk |
| space | jump, swim up, fly up |
| shift | sneak, fly down |
| ctrl | run |
| left, right button | break, place a block (held: again every quarter second) |
| middle button | put the block you look at in the hotbar |
| 1-9, wheel | choose a block |
| F | fly on or off |
| F2 | screenshot, to `quarry-N.ppm` |
| F3 | position and frame rate on or off |
| Esc | free the mouse; Esc again quits |

`--shot FILE` draws one frame to a PPM image and quits. With SDL's offscreen driver this
needs no screen, though it still needs OpenGL: a GPU, or Mesa. `--at`, `--look` and `--fly`
place the camera:

```
SDL_VIDEODRIVER=offscreen zink run -O examples/quarry/quarry.zn \
    --shot view.ppm --at 20,95,236 --look 45,28 --fly
```

The compiler's tests build quarry where SDL2 and OpenGL's headers are installed, and draw a
frame this way where SDL can make an OpenGL context.

## How it reaches C

`extern import SDL2/SDL.h;` and `extern import GL/gl.h;` read the headers (DESIGN §12.6).
The 41 functions it calls that take only numbers, text and handles are called as they are,
with their constants: `SDL_CreateWindow` returns an `SDL_Window` handle, and `glRotated`,
`GL_DEPTH_TEST` and `SDL_SCANCODE_W` work as written.

The other 11 take pointers. Each has an explicit `extern` declaration that says what its
pointer means:
- a slice of numbers passes its pointer, so `glBufferData` gets the vertex array and
  `glTexImage2D` the pixels;
- a writable view is one C may write into: `glGenBuffers` writes into a `u32` array,
  `glReadPixels` into the screenshot's bytes, and `SDL_PollEvent` into an array of 16
  `u32`s, which then serves as the `SDL_Event`. Its fields are read by their offsets:
  the type at 0, a key's scancode at byte 16, the mouse's motion at bytes 28 and 32;
- `SDL_GL_CreateContext` returns a `void *`, declared as a `u64`, since the program only
  checks it isn't 0;
- `glInterleavedArrays` takes a pointer that, with a vertex buffer bound, is an offset
  into the buffer: a `u64` 0.

The drawing is OpenGL 1.5's fixed pipeline, with a vertex buffer per chunk in
`GL_T2F_C3F_V3F` layout. Shaders would need `glShaderSource`, whose source is a
`char **`, an array of pointers to text, which nothing in Zinkite can make.

## How it's built

- **The world** is a `root` (DESIGN §6.6) with two growable parts: the blocks, a byte
  each, and the height of each column's highest block that shades. It is generated in
  four steps: columns from noise, worm caves, ore veins, then trees and plants.
- **Meshes:** each 16 × 16 chunk has two vertex buffers. One holds the cubes and plants,
  with see-through pixels cut by the alpha test. The other holds the water, drawn
  blended after everything it may cover. Only faces next to something see-through are
  built, each corner shaded by its ambient occlusion and the sky light in front of the
  face. A changed block rebuilds its chunk, and the neighbouring one when it's on the
  edge.
- **Moving:** the player is a box 0.6 wide and 1.8 tall, moved one axis at a time, in
  steps short enough not to pass through a block, and stopped against the first block
  in the way.
- **Aiming:** a ray walks the grid cell by cell (Amanatides and Woo) to the first block
  within reach. It fails with `MISSING` when there's none, and `Hit aimed @nothing =
  aim(...)` reads that.
- **Loading** a world reads its numbers and runs of blocks without checking each step.
  A short or damaged file fails wherever it first reads past the end or misparses, and
  `u8 bad = @@world.load(data, player);` in `main` catches that failure.
