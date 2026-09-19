# ============================================================
# Oxenna Makefile
# ============================================================

SHELL := /bin/bash

# ============================================================
# Colors
# ============================================================

RESET  := \033[0m
BOLD   := \033[1m

RED    := \033[31m
GREEN  := \033[32m
YELLOW := \033[33m
BLUE   := \033[34m
CYAN   := \033[36m
GRAY   := \033[90m

# ============================================================
# Paths
# ============================================================

KERNEL_TARGET := x86_64-unknown-none

OUT := out

# Keep all generated build artifacts under out/.
CARGO_TARGET_DIR := $(OUT)/target
TEST_TARGET_DIR  := $(OUT)/target-test

KERNEL := $(CARGO_TARGET_DIR)/$(KERNEL_TARGET)/debug/oxenna
TEST_KERNEL := $(TEST_TARGET_DIR)/$(KERNEL_TARGET)/debug/oxenna

ISO := $(OUT)/oxenna.iso
TEST_ISO := $(OUT)/oxenna_tests.iso
DISK := $(OUT)/disk.img

ISO_ROOT := $(OUT)/iso_root

LIMINE_DIR := limine
LIMINE := $(LIMINE_DIR)/limine

LIMINE_REPO := https://github.com/limine-bootloader/limine.git
LIMINE_BRANCH := v9.x-binary

USER_SRC := user/user.asm
USER_BIN := $(OUT)/user.bin

DISK_SIZE := 64M

# ============================================================
# Helpers
# ============================================================

define banner
	@printf "\n$(BOLD)$(CYAN)╔══════════════════════════════════════════════════════╗$(RESET)\n"
	@printf "$(BOLD)$(CYAN)║  %-52s║$(RESET)\n" "$(1)"
	@printf "$(BOLD)$(CYAN)╚══════════════════════════════════════════════════════╝$(RESET)\n"
endef

define step
	@printf "$(BOLD)$(BLUE)  ▶$(RESET) %s\n" "$(1)"
endef

define success
	@printf "$(BOLD)$(GREEN)  ✓$(RESET) %s\n" "$(1)"
endef

.PHONY: all clean cleaniso rebuild
.PHONY: kernel kernel-tests user
.PHONY: iso test-iso disk run test
.PHONY: limine

# ============================================================
# Default
# ============================================================

all: $(ISO) $(TEST_ISO) $(DISK)
	$(call success,Build complete!)

# ============================================================
# Limine
# ============================================================

$(LIMINE):
	$(call step,Building Limine...)

	@if [ ! -d "$(LIMINE_DIR)" ]; then \
		printf "$(BOLD)$(YELLOW)  !$(RESET) Limine not found, cloning...\n"; \
		git clone \
			--branch $(LIMINE_BRANCH) \
			--depth 1 \
			$(LIMINE_REPO) \
			$(LIMINE_DIR); \
	fi

	@$(MAKE) -C $(LIMINE_DIR)

	$(call success,Limine ready)

limine: $(LIMINE)

# ============================================================
# Normal kernel
# ============================================================

kernel:
	$(call banner,BUILDING KERNEL)

	$(call step,Compiling kernel...)
	@mkdir -p $(OUT)
	@CARGO_TARGET_DIR=$(CARGO_TARGET_DIR) cargo build

	$(call success,Kernel: $(KERNEL))

# ============================================================
# Test kernel
# ============================================================

kernel-tests:
	$(call banner,BUILDING TEST KERNEL)

	$(call step,Compiling kernel with test feature...)
	@mkdir -p $(OUT)
	@CARGO_TARGET_DIR=$(TEST_TARGET_DIR) \
		cargo build --features test

	$(call success,Test kernel: $(TEST_KERNEL))

# ============================================================
# Userspace
# ============================================================

user: $(USER_BIN)

$(USER_BIN): $(USER_SRC)
	$(call banner,BUILDING USERSPACE)
	@mkdir -p $(OUT)

	$(call step,Assembling $(USER_SRC)...)
	@nasm -f bin $(USER_SRC) -o $(USER_BIN)

	$(call success,Userspace: $(USER_BIN))

# ============================================================
# Ext2 disk image
# ============================================================

disk: $(DISK)

$(DISK):
	$(call banner,CREATING EXT2 DISK)

	$(call step,Creating $(DISK_SIZE) disk image...)
	@mkdir -p $(OUT)
	@qemu-img create -f raw $(DISK) $(DISK_SIZE) >/dev/null

	$(call step,Formatting disk as ext2...)
	@mkfs.ext2 -F $(DISK) >/dev/null

	$(call success,Ext2 disk: $(DISK))

# ============================================================
# Normal ISO
# ============================================================

$(ISO): kernel user $(LIMINE)
	$(call banner,BUILDING NORMAL ISO)

	@rm -rf $(ISO_ROOT)

	@mkdir -p $(ISO_ROOT)/boot
	@mkdir -p $(ISO_ROOT)/EFI/BOOT

	$(call step,Copying kernel...)
	@cp $(KERNEL) $(ISO_ROOT)/boot/oxenna

	$(call step,Copying userspace...)
	@cp $(USER_BIN) $(ISO_ROOT)/boot/user.bin

	$(call step,Copying Limine configuration...)
	@cp limine.conf $(ISO_ROOT)/limine.conf

	$(call step,Copying Limine boot files...)
	@cp $(LIMINE_DIR)/limine-bios-cd.bin $(ISO_ROOT)/boot/
	@cp $(LIMINE_DIR)/limine-uefi-cd.bin $(ISO_ROOT)/boot/
	@cp $(LIMINE_DIR)/BOOTX64.EFI $(ISO_ROOT)/EFI/BOOT/
	@cp $(LIMINE_DIR)/limine-bios.sys $(ISO_ROOT)/boot/

	$(call step,Creating ISO...)
	@xorriso -as mkisofs \
		-R -r -J \
		-b boot/limine-bios-cd.bin \
		-no-emul-boot \
		-boot-load-size 4 \
		-boot-info-table \
		--efi-boot boot/limine-uefi-cd.bin \
		-efi-boot-part \
		--efi-boot-image \
		-o $(ISO) \
		$(ISO_ROOT) \
		>/dev/null 2>&1

	$(call step,Installing Limine...)
	@$(LIMINE) bios-install $(ISO) >/dev/null

	$(call success,Normal ISO: $(ISO))

iso: $(ISO)

# ============================================================
# Test ISO
# ============================================================

$(TEST_ISO): kernel-tests $(LIMINE)
	$(call banner,BUILDING TEST ISO)

	@rm -rf $(ISO_ROOT)

	@mkdir -p $(ISO_ROOT)/boot
	@mkdir -p $(ISO_ROOT)/EFI/BOOT

	$(call step,Copying TEST kernel...)
	@cp $(TEST_KERNEL) $(ISO_ROOT)/boot/oxenna

	$(call step,Copying TEST Limine configuration...)
	@cp limine_test.conf $(ISO_ROOT)/limine.conf

	$(call step,Copying Limine boot files...)
	@cp $(LIMINE_DIR)/limine-bios-cd.bin $(ISO_ROOT)/boot/
	@cp $(LIMINE_DIR)/limine-uefi-cd.bin $(ISO_ROOT)/boot/
	@cp $(LIMINE_DIR)/BOOTX64.EFI $(ISO_ROOT)/EFI/BOOT/
	@cp $(LIMINE_DIR)/limine-bios.sys $(ISO_ROOT)/boot/

	$(call step,Creating TEST ISO...)
	@xorriso -as mkisofs \
		-R -r -J \
		-b boot/limine-bios-cd.bin \
		-no-emul-boot \
		-boot-load-size 4 \
		-boot-info-table \
		--efi-boot boot/limine-uefi-cd.bin \
		-efi-boot-part \
		--efi-boot-image \
		-o $(TEST_ISO) \
		$(ISO_ROOT) \
		>/dev/null 2>&1

	$(call step,Installing Limine...)
	@$(LIMINE) bios-install $(TEST_ISO) >/dev/null

	$(call success,Test ISO: $(TEST_ISO))

test-iso: $(TEST_ISO)

# ============================================================
# Normal run
# ============================================================

run: $(ISO) $(DISK)
	$(call banner,BOOTING OXENNA)

	@qemu-system-x86_64 \
		-cdrom $(ISO) \
		-m 256M \
		-serial stdio \
		-monitor none \
		-drive file=$(DISK),format=raw,if=ide \
		-device isa-debug-exit,iobase=0xf4,iosize=0x04

# ============================================================
# Tests
# ============================================================

test: $(TEST_ISO) $(DISK)
	$(call banner,RUNNING KERNEL TESTS)

	@printf "$(GRAY)  Kernel: $(TEST_KERNEL)$(RESET)\n"
	@printf "$(GRAY)  ISO:    $(TEST_ISO)$(RESET)\n"
	@printf "$(GRAY)  Disk:   $(DISK)$(RESET)\n\n"

	@set +e; \
	qemu-system-x86_64 \
		-cdrom $(TEST_ISO) \
		-m 256M \
		-serial stdio \
		-monitor none \
		-drive file=$(DISK),format=raw,if=ide \
		-device isa-debug-exit,iobase=0xf4,iosize=0x04; \
	status=$$?; \
	printf "\n"; \
	if [ $$status -eq 33 ]; then \
		printf "$(BOLD)$(GREEN)  ✓ ALL TESTS PASSED$(RESET)\n"; \
		exit 0; \
	elif [ $$status -eq 35 ]; then \
		printf "$(BOLD)$(RED)  ✗ TESTS FAILED$(RESET)\n"; \
		exit 1; \
	else \
		printf "$(BOLD)$(RED)  ✗ QEMU EXITED UNEXPECTEDLY$(RESET)\n"; \
		printf "$(GRAY)    status = %s$(RESET)\n" "$$status"; \
		exit $$status; \
	fi

# ============================================================
# Cleaning
# ============================================================

clean:
	$(call banner,CLEANING)

	$(call step,Removing all generated output...)
	@rm -rf $(OUT)

	$(call success,Clean complete)

cleaniso:
	$(call banner,CLEANING ISOS)

	@rm -f $(ISO) $(TEST_ISO)
	@rm -rf $(ISO_ROOT)

	$(call success,ISO files removed)

# ============================================================
# Full rebuild
# ============================================================

rebuild:
	$(MAKE) clean
	$(MAKE) all

.DEFAULT_GOAL := all
