KERNEL := target/x86_64-unknown-none/debug/oxenna

ISO := oxenna.iso
TEST_ISO := oxenna_tests.iso

USER_SRC := user/user.asm
USER_BIN := user.bin

LIMINE_DIR := limine
LIMINE := $(LIMINE_DIR)/limine

LIMINE_REPO := https://github.com/limine-bootloader/limine.git
LIMINE_BRANCH := v9.x-binary

.PHONY: all clean run test kernel kernel-tests limine cleaniso

all: $(ISO)


# ============================================================
# Limine
# ============================================================

$(LIMINE):
	@if [ ! -d "$(LIMINE_DIR)" ]; then \
		echo "Limine directory not found. Cloning Limine..."; \
		git clone --branch $(LIMINE_BRANCH) --depth 1 $(LIMINE_REPO) $(LIMINE_DIR); \
	fi
	$(MAKE) -C $(LIMINE_DIR)


limine: $(LIMINE)


# ============================================================
# Kernel
# ============================================================

kernel:
	cargo build


kernel-tests:
	cargo build --features test


# ============================================================
# Normal ISO
# ============================================================

$(ISO): kernel user $(LIMINE)
	rm -rf iso_root

	mkdir -p iso_root/boot
	mkdir -p iso_root/EFI/BOOT

	cp $(KERNEL) iso_root/boot/oxenna
	cp limine.conf iso_root/limine.conf

	cp $(USER_BIN) iso_root/boot/user.bin

	cp $(LIMINE_DIR)/limine-bios-cd.bin iso_root/boot/
	cp $(LIMINE_DIR)/limine-uefi-cd.bin iso_root/boot/
	cp $(LIMINE_DIR)/BOOTX64.EFI iso_root/EFI/BOOT/
	cp $(LIMINE_DIR)/limine-bios.sys iso_root/boot/

	xorriso -as mkisofs \
		-R -r -J \
		-b boot/limine-bios-cd.bin \
		-no-emul-boot \
		-boot-load-size 4 \
		-boot-info-table \
		--efi-boot boot/limine-uefi-cd.bin \
		-efi-boot-part \
		--efi-boot-image \
		--protective-msdos-label \
		iso_root \
		-o $(ISO)

	$(LIMINE) bios-install $(ISO)


# ============================================================
# Test ISO
# ============================================================

$(TEST_ISO): kernel-tests $(LIMINE)
	rm -rf iso_root

	mkdir -p iso_root/boot
	mkdir -p iso_root/EFI/BOOT

	cp $(KERNEL) iso_root/boot/oxenna
	cp limine.conf iso_root/limine.conf

	cp $(LIMINE_DIR)/limine-bios-cd.bin iso_root/boot/
	cp $(LIMINE_DIR)/limine-uefi-cd.bin iso_root/boot/
	cp $(LIMINE_DIR)/BOOTX64.EFI iso_root/EFI/BOOT/
	cp $(LIMINE_DIR)/limine-bios.sys iso_root/boot/

	xorriso -as mkisofs \
		-R -r -J \
		-b boot/limine-bios-cd.bin \
		-no-emul-boot \
		-boot-load-size 4 \
		-boot-info-table \
		--efi-boot boot/limine-uefi-cd.bin \
		-efi-boot-part \
		--efi-boot-image \
		--protective-msdos-label \
		iso_root \
		-o $(TEST_ISO)

	$(LIMINE) bios-install $(TEST_ISO)

# ============================================================
# User-space binaries
# ============================================================

user: $(USER_BIN)

$(USER_BIN): $(USER_SRC)
	mkdir -p user
	nasm -f bin $(USER_SRC) -o $(USER_BIN)

# ============================================================
# Run
# ============================================================

run: $(ISO)
	@printf '\033[2J\033[H'
	@qemu-system-x86_64 \
		-m 512M \
		-cdrom $(ISO) \
		-serial stdio \
		-device isa-debug-exit,iobase=0xf4,iosize=0x04; \
	status=$$?; \
	if [ $$status -eq 33 ]; then \
		exit 0; \
	elif [ $$status -eq 35 ]; then \
		exit 1; \
	else \
		echo "QEMU exited unexpectedly with code $$status"; \
		exit 1; \
	fi
# ============================================================
# Tests
# ============================================================

test: $(TEST_ISO)
	@printf '\033[2J\033[H'
	@qemu-system-x86_64 \
		-cdrom $(TEST_ISO) \
		-m 256M \
		-display none \
		-serial stdio \
		-monitor none \
		-device isa-debug-exit,iobase=0xf4,iosize=0x04; \
	status=$$?; \
	if [ $$status -eq 33 ]; then \
		exit 0; \
	elif [ $$status -eq 35 ]; then \
		exit 1; \
	else \
		echo "QEMU exited unexpectedly with code $$status"; \
		exit 1; \
	fi


# ============================================================
# Clean
# ============================================================

clean:
	cargo clean
	rm -rf iso_root
	rm -f $(ISO)
	rm -f $(TEST_ISO)
	rm -rf $(LIMINE_DIR)


cleaniso:
	rm -rf iso_root
	rm -f $(ISO)
	rm -f $(TEST_ISO)
