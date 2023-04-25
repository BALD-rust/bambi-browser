MEMORY
{
  /* NOTE 1 K = 1 KiBi = 1024 bytes */
  MBR                               : ORIGIN = 0x00000000, LENGTH = 4K
  SOFTDEVICE                        : ORIGIN = 0x00001000, LENGTH = 152K
  FLASH                             : ORIGIN = 0x00027000, LENGTH = 850K
  RAM                         (rwx) : ORIGIN = 0x20000008, LENGTH = 262136
}