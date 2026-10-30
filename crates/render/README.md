# Renderer

`render_item_lite` box-reduces planar samples before colour conversion for Source
playback at half resolution or lower. The exact renderer shares the implementation
with this path disabled. Sequence and RGBA items retain the exact path.

Synthetic tests cover exact equivalence and a 38 dB minimum RGB quality threshold
for reduced 10-bit 4:2:2. Execution is unverified without a Rust toolchain.
