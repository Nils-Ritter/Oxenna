# ============================================================
# Oxenna Makefile
# ============================================================

SHELL := /bin/bash
.DEFAULT_GOAL := all

KERNEL_TARGET := x86_64-unknown-none
OUT := out
CARGO_TARGET_DIR := $(OUT)/target
TEST_TARGET_DIR := $(OUT)/target-test

CONFIG_MK := $(OUT)/config/config.mk
KCONFIG := Kconfig
KCONFIG_TOOL := scripts/kconfig.py
# Bootstrap defaults are used only until config.mk exists.
CONFIG_BUILD_KERNEL ?= y
CONFIG_BUILD_ISO ?= y
CONFIG_BUILD_DISK ?= y
CONFIG_INSTALL_APPS ?= y
CONFIG_BUILD_TESTS ?= n
CONFIG_BUILD_TEST_ISO ?= n
CONFIG_ELF_SUPPORT ?= y
CONFIG_APP_HELLO ?= y
CONFIG_APP_FASTFETCH ?= y
-include $(CONFIG_MK)

KERNEL_PROFILE := $(if $(filter y,$(CONFIG_RELEASE_BUILD)),release,debug)
CARGO_PROFILE_ARGS := $(if $(filter y,$(CONFIG_RELEASE_BUILD)),--release,)
CARGO_DEBUG_ARGS := $(if $(filter y,$(CONFIG_DEBUG_INFO)),,--config profile.dev.debug=0 --config profile.release.debug=0)
KERNEL := $(CARGO_TARGET_DIR)/$(KERNEL_TARGET)/$(KERNEL_PROFILE)/oxenna
TEST_KERNEL := $(TEST_TARGET_DIR)/$(KERNEL_TARGET)/debug/oxenna

ISO := $(OUT)/oxenna.iso
TEST_ISO := $(OUT)/oxenna_tests.iso
DISK := disk.img
ISO_ROOT := $(OUT)/iso_root

LIMINE_DIR := limine
LIMINE := $(LIMINE_DIR)/limine
LIMINE_REPO := https://github.com/limine-bootloader/limine.git
LIMINE_BRANCH := v9.x-binary

APP_SRC := $(wildcard apps/*.S)
APP_BINS := $(patsubst apps/%.S,$(OUT)/apps/%.ox,$(APP_SRC))
ENABLED_APP_BINS :=
ifeq ($(CONFIG_APP_HELLO),y)
ENABLED_APP_BINS += $(OUT)/apps/hello.ox
endif
ifeq ($(CONFIG_APP_FASTFETCH),y)
ENABLED_APP_BINS += $(OUT)/apps/fastfetch.ox
endif
DISK_SIZE := 64M

RESET := \033[0m
BOLD := \033[1m
RED := \033[31m
GREEN := \033[32m
YELLOW := \033[33m
BLUE := \033[34m
CYAN := \033[36m
GRAY := \033[90m

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

.PHONY: all __build config menuconfig olddefconfig defconfig
.PHONY: kernel kernel-tests apps disk-apps disk-install
.PHONY: iso test-iso disk run test limine
.PHONY: clean cleaniso rebuild

# ============================================================
# Configuration
# ============================================================

config:
	@python3 $(KCONFIG_TOOL) sync

menuconfig:
	@python3 $(KCONFIG_TOOL) menuconfig

olddefconfig:
	@python3 $(KCONFIG_TOOL) olddefconfig

defconfig:
	@rm -f .config
	@python3 $(KCONFIG_TOOL) olddefconfig

# ============================================================
# Default build
# ============================================================

BUILD_TARGETS :=
ifeq ($(CONFIG_BUILD_KERNEL),y)
ifeq ($(CONFIG_BUILD_ISO),y)
BUILD_TARGETS += $(ISO)
else
BUILD_TARGETS += kernel
endif
endif
ifeq ($(CONFIG_BUILD_DISK),y)
BUILD_TARGETS += disk
ifeq ($(CONFIG_INSTALL_APPS),y)
BUILD_TARGETS += disk-apps
endif
endif
ifeq ($(CONFIG_BUILD_TESTS),y)
ifeq ($(CONFIG_BUILD_TEST_ISO),y)
BUILD_TARGETS += $(TEST_ISO)
else
BUILD_TARGETS += kernel-tests
endif
endif

all: config
	@$(MAKE) --no-print-directory __build

__build:
	@$(MAKE) --no-print-directory $(BUILD_TARGETS)
	$(call success,Build complete according to .config)

# ============================================================
# Limine
# ============================================================

$(LIMINE):
	$(call step,Building Limine...)
	@if [ ! -d "$(LIMINE_DIR)" ]; then \
		printf "$(BOLD)$(YELLOW)  !$(RESET) Limine not found, cloning...\n"; \
		git clone --branch $(LIMINE_BRANCH) --depth 1 $(LIMINE_REPO) $(LIMINE_DIR); \
	fi
	@$(MAKE) -C $(LIMINE_DIR)
	$(call success,Limine ready)

limine: $(LIMINE)

# ============================================================
# Kernel
# ============================================================

kernel: config
ifeq ($(CONFIG_BUILD_KERNEL),y)
	$(call banner,BUILDING KERNEL)
	$(call step,Compiling kernel...)
	@mkdir -p $(OUT)
	@CARGO_TARGET_DIR=$(CARGO_TARGET_DIR) cargo build --no-default-features $(CARGO_PROFILE_ARGS) $(CARGO_DEBUG_ARGS) $(CARGO_FEATURE_ARGS)
	$(call success,Kernel: $(KERNEL))
else
	@printf "Kernel build disabled by CONFIG_BUILD_KERNEL=n\n"
endif

# ============================================================
# Test kernel
# ============================================================

kernel-tests: config
ifeq ($(CONFIG_BUILD_TESTS),y)
	$(call banner,BUILDING TEST KERNEL)
	$(call step,Compiling kernel with tests...)
	@mkdir -p $(TEST_TARGET_DIR)
	@CARGO_TARGET_DIR=$(TEST_TARGET_DIR) cargo build --no-default-features --features "$(strip $(CARGO_FEATURES)) test"
	$(call success,Test kernel: $(TEST_KERNEL))
else
	@printf "Kernel tests disabled by CONFIG_BUILD_TESTS=n\n"
endif

# ============================================================
# Standalone .ox applications
# ============================================================

apps: config
ifeq ($(CONFIG_ELF_SUPPORT),y)
	@$(MAKE) --no-print-directory $(ENABLED_APP_BINS)
else
	@printf "Userspace/ELF support is disabled; no applications will be built.\n"
endif

$(OUT)/apps/%.ox: apps/%.S
	$(call banner,BUILDING APP $*)
	@mkdir -p $(OUT)/apps
	$(call step,Assembling and linking ELF64 ET_DYN executable...)
	@gcc -c -fPIE -m64 $< -o $(OUT)/apps/$*.o
	@ld -pie --no-dynamic-linker --build-id=none -z max-page-size=0x1000 -e _start -o $@ $(OUT)/apps/$*.o
	@readelf -h $@ | grep -q 'Class:.*ELF64' || (echo "error: $@ is not ELF64"; exit 1)
	@readelf -h $@ | grep -q 'Type:.*DYN' || (echo "error: $@ is not ET_DYN"; exit 1)
	@if readelf -l $@ | grep -q INTERP; then echo "error: $@ contains PT_INTERP"; exit 1; fi
	$(call success,Application: $@)

# ============================================================
# Ext2 disk image
# ============================================================

disk: config
ifeq ($(CONFIG_BUILD_DISK),y)
	@if [ ! -f "$(DISK)" ]; then $(MAKE) --no-print-directory $(DISK); else printf "Disk image already exists: $(DISK)\n"; fi
else
	@printf "Disk image build disabled by CONFIG_BUILD_DISK=n\n"
endif

$(DISK):
	$(call banner,CREATING EXT2 DISK)
	$(call step,Creating $(DISK_SIZE) disk image...)
	@qemu-img create -f raw $(DISK) $(DISK_SIZE) >/dev/null
	$(call step,Formatting disk as ext2...)
	@mkfs.ext2 -F $(DISK) >/dev/null
	$(call success,Ext2 disk: $(DISK))

ifeq ($(CONFIG_INSTALL_APPS),y)
disk-apps: config disk apps
	$(call banner,INSTALLING SELECTED .OX APPLICATIONS)
	@if [ -n "$(strip $(ENABLED_APP_BINS))" ]; then \
		debugfs -w -R "mkdir /bin" $(DISK) >/dev/null 2>&1 || true; \
		for app in $(ENABLED_APP_BINS); do \
			name=$$(basename "$$app"); \
			printf "  Installing %s -> /bin/%s\\n" "$$app" "$$name"; \
			debugfs -w -R "write $$app /bin/$$name" $(DISK) >/dev/null; \
		done; \
	else \
		printf "No applications are enabled.\\n"; \
	fi
	$(call success,Selected applications installed)
else
disk-apps: config
	@printf "Application installation disabled by CONFIG_INSTALL_APPS=n\n"
endif

disk-install: disk
	@test -n "$(APP)" || (echo "Usage: make disk-install APP=foo.ox [DEST=/bin/foo.ox]"; exit 2)
	@debugfs -w -R "write $(APP) $(if $(DEST),$(DEST),/$(notdir $(APP)))" $(DISK) >/dev/null
	$(call success,Installed $(APP) -> $(if $(DEST),$(DEST),/$(notdir $(APP))))

# ============================================================
# Normal ISO
# ============================================================

ifeq ($(CONFIG_BUILD_ISO),y)
$(ISO): kernel $(LIMINE)
	$(call banner,BUILDING NORMAL ISO)
	@rm -rf $(ISO_ROOT)
	@mkdir -p $(ISO_ROOT)/boot $(ISO_ROOT)/EFI/BOOT
	$(call step,Copying kernel...)
	@cp $(KERNEL) $(ISO_ROOT)/boot/oxenna
	$(call step,Copying Limine configuration...)
ifeq ($(CONFIG_ELF_SUPPORT),y)
	@cp limine.conf $(ISO_ROOT)/limine.conf
else
	@sed '/^[[:space:]]*module_path:/d' limine.conf > $(ISO_ROOT)/limine.conf
endif
	$(call step,Copying Limine boot files...)
	@cp $(LIMINE_DIR)/limine-bios-cd.bin $(ISO_ROOT)/boot/
	@cp $(LIMINE_DIR)/limine-uefi-cd.bin $(ISO_ROOT)/boot/
	@cp $(LIMINE_DIR)/BOOTX64.EFI $(ISO_ROOT)/EFI/BOOT/
	@cp $(LIMINE_DIR)/limine-bios.sys $(ISO_ROOT)/boot/
	$(call step,Creating ISO...)
	@xorriso -as mkisofs -R -r -J \
		-b boot/limine-bios-cd.bin -no-emul-boot -boot-load-size 4 -boot-info-table \
		--efi-boot boot/limine-uefi-cd.bin -efi-boot-part --efi-boot-image \
		-o $(ISO) $(ISO_ROOT) >/dev/null 2>&1
	$(call step,Installing Limine...)
	@$(LIMINE) bios-install $(ISO) >/dev/null
	$(call success,Normal ISO: $(ISO))
else
$(ISO):
	@printf "ISO build disabled by CONFIG_BUILD_ISO=n\n"
endif

iso: $(ISO)

# ============================================================
# Test ISO
# ============================================================

ifeq ($(CONFIG_BUILD_TESTS),y)
ifeq ($(CONFIG_BUILD_TEST_ISO),y)
$(TEST_ISO): kernel-tests $(LIMINE)
	$(call banner,BUILDING TEST ISO)
	@rm -rf $(ISO_ROOT)
	@mkdir -p $(ISO_ROOT)/boot $(ISO_ROOT)/EFI/BOOT
	$(call step,Copying test kernel...)
	@cp $(TEST_KERNEL) $(ISO_ROOT)/boot/oxenna
	$(call step,Copying test Limine configuration...)
	@cp limine_test.conf $(ISO_ROOT)/limine.conf
	$(call step,Copying Limine boot files...)
	@cp $(LIMINE_DIR)/limine-bios-cd.bin $(ISO_ROOT)/boot/
	@cp $(LIMINE_DIR)/limine-uefi-cd.bin $(ISO_ROOT)/boot/
	@cp $(LIMINE_DIR)/BOOTX64.EFI $(ISO_ROOT)/EFI/BOOT/
	@cp $(LIMINE_DIR)/limine-bios.sys $(ISO_ROOT)/boot/
	$(call step,Creating test ISO...)
	@xorriso -as mkisofs -R -r -J \
		-b boot/limine-bios-cd.bin -no-emul-boot -boot-load-size 4 -boot-info-table \
		--efi-boot boot/limine-uefi-cd.bin -efi-boot-part --efi-boot-image \
		-o $(TEST_ISO) $(ISO_ROOT) >/dev/null 2>&1
	$(call step,Installing Limine...)
	@$(LIMINE) bios-install $(TEST_ISO) >/dev/null
	$(call success,Test ISO: $(TEST_ISO))
endif
endif

test-iso: $(TEST_ISO)

# ============================================================
# Run / test
# ============================================================

run: $(ISO) disk
	$(call banner,BOOTING OXENNA)
	@qemu-system-x86_64 -cdrom $(ISO) -m 256M -serial stdio -monitor none \
		-drive file=$(DISK),format=raw,if=ide \
		-device isa-debug-exit,iobase=0xf4,iosize=0x04

test: $(TEST_ISO) disk
	$(call banner,RUNNING KERNEL TESTS)
	@set +e; \
	qemu-system-x86_64 -cdrom $(TEST_ISO) -m 256M -serial stdio -monitor none \
		-drive file=$(DISK),format=raw,if=ide \
		-device isa-debug-exit,iobase=0xf4,iosize=0x04; \
	status=$$?; \
	if [ $$status -eq 33 ]; then printf "$(BOLD)$(GREEN)  ✓ ALL TESTS PASSED$(RESET)\n"; exit 0; \
	elif [ $$status -eq 35 ]; then printf "$(BOLD)$(RED)  ✗ TESTS FAILED$(RESET)\n"; exit 1; \
	else printf "$(BOLD)$(RED)  ✗ QEMU EXITED UNEXPECTEDLY$(RESET) status=%s\n" "$$status"; exit $$status; fi

# ============================================================
# Cleaning
# ============================================================

clean:
	$(call banner,CLEANING)
	$(call step,Removing generated output and disk image...)
	@rm -rf $(OUT) $(DISK)
	$(call success,Clean complete)

cleaniso:
	@rm -f $(ISO) $(TEST_ISO)
	@rm -rf $(ISO_ROOT)
	$(call success,ISO files removed)

rebuild:
	$(MAKE) clean
	$(MAKE) all
