/* K210 general SRAM is 6 MiB at 0x80000000 (Renode maps 32 MiB there). The
   ELF is loaded straight into RAM, so "FLASH" is just the lower half. */
MEMORY
{
  FLASH : ORIGIN = 0x80000000, LENGTH = 3M
  RAM   : ORIGIN = 0x80300000, LENGTH = 3M
}
REGION_ALIAS("REGION_TEXT", FLASH);
REGION_ALIAS("REGION_RODATA", FLASH);
REGION_ALIAS("REGION_DATA", RAM);
REGION_ALIAS("REGION_BSS", RAM);
REGION_ALIAS("REGION_HEAP", RAM);
REGION_ALIAS("REGION_STACK", RAM);

/* riscv-rt's link.x keeps .eh_frame as an (INFO) section at address 0. Its
   32-bit PC-relative references to code at 0x80000000 then overflow on RV64
   (on RV32 they wrap, so the FE310 is fine). Nothing here unwinds
   (panic-halt), so drop the tables. This script is read before link.x, so the
   discard wins. */
SECTIONS
{
  /DISCARD/ : { *(.eh_frame) *(.eh_frame_hdr) }
}
