MEMORY
{
  FLASH : ORIGIN = 0x00000000, LENGTH = 1024K
  RAM : ORIGIN = 0x20000000, LENGTH = 256K
}

SECTIONS
{
  /* Reset-persistent results: NOT re-initialized by cortex-m-rt startup, so
     values written before a SYSRESETREQ reboot survive into the next boot.
     The provisioning boot (boot 1) explicitly zeroes them. */
  .uninit (NOLOAD) :
  {
    *(.uninit .uninit.*);
  } > RAM
}
