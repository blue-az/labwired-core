# Required Source Documents (Feather-STM32F405)

## MCU Reference Manual + Datasheet

1. RM0090 — STM32F405/407/415/417 reference manual (shared F4 IP; the F405
   descriptor reuses the `stm32f4` / `stm32v2` profiles):
   https://www.st.com/resource/en/reference_manual/dm00031020.pdf
2. DS8626 — STM32F405RG datasheet (memory sizes, package):
   https://www.st.com/resource/en/datasheet/stm32f405rg.pdf

## MCU (CMSIS Device Header)

1. STM32F405 device header (memory map, base addresses, IRQs):
   https://github.com/STMicroelectronics/cmsis_device_f4/blob/master/Include/stm32f405xx.h

## DBGMCU Identity

1. RM0090 §38.6.1 — DBGMCU_IDCODE: DEV_ID = 0x413 for STM32F405/407
   (REV_ID = 0x1000 → idcode 0x10006413).
