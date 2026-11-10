/* Linked into the Linux release binary only (`cargo xtask size`/`dist`, the source PKGBUILD):
 * its unwind tables and landing pads, which a binary that aborts on panic never uses. See
 * `release_link_args` in xtask/src/main.rs. GNU ld (bfd) and lld take it; gold and mold do not. */
SECTIONS { /DISCARD/ : { *(.eh_frame) *(.eh_frame_hdr) *(.gcc_except_table .gcc_except_table.*) } } INSERT AFTER .text;
