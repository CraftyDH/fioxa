if exist \EFI\BOOT\BOOTx64.efi then
 \EFI\BOOT\BOOTx64.efi
 goto END
endif

if exist fs0:\EFI\BOOT\BOOTx64.efi then
 fs0:\EFI\BOOT\BOOTx64.efi
 goto END
endif

:END
