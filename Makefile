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

KERNEL := target/$(KERNEL_TARGET)/debug/oxenna

TEST_TARGET_DIR := target-test
TEST_KERNEL := $(TEST_TARGET_DIR)/$(KERNEL_TARGET)/debug/oxenna

ISO := oxenna.iso
TEST_ISO := oxenna_tests.iso

ISO_ROOT := iso_root

LIMINE_DIR := limine
LIMINE := $(LIMINE_DIR)/limine

LIMINE_REPO := https://github.com/limine-bootloader/limine.git
LIMINE_BRANCH := v9.x-binary

USER_SRC := user/user.oxs
USER_BIN := userbin/user.bin


# ============================================================
# Terminal UI
# ============================================================

UI := ./scripts/oxenna-ui.sh

UI_DIR := .oxenna-ui
UI_LOG := $(UI_DIR)/output.log
UI_STATE := $(UI_DIR)/state
UI_ACTIVE := $(UI_DIR)/active
UI_PID := $(UI_DIR)/renderer.pid


# ============================================================
# UI lifecycle
#
# The renderer is started ONCE for a complete make invocation.
#
# Stages only update the state file.
# ============================================================

define UI_START
	@mkdir -p "$(UI_DIR)"
	@rm -f "$(UI_ACTIVE)" "$(UI_PID)"
	@touch "$(UI_ACTIVE)"
	@printf '%s|%s|%s\n' "$(1)" "$(2)" "$(3)" > "$(UI_STATE)"
    @chmod +x $(UI)
	@$(UI) start "$(1)" "$(2)" "$(3)" &
	@echo $$! > "$(UI_PID)"
	@sleep 0.12
endef


define UI_STAGE
	@printf '%s|%s|%s\n' "$(1)" "$(2)" "$(3)" > "$(UI_STATE)"
endef


define UI_STOP
	@$(UI) stop >/dev/null 2>&1 || true
	@sleep 0.15
endef


# ============================================================
# Run a noisy command
# ============================================================

define UI_RUN
	@printf '\n' >> "$(UI_LOG)"
	@printf '━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n' >> "$(UI_LOG)"
	@printf '  %s\n' "$(1)" >> "$(UI_LOG)"
	@printf '━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n\n' >> "$(UI_LOG)"
	@set -o pipefail; \
		( $(2) ) >> "$(UI_LOG)" 2>&1; \
	status=$$?; \
	if [ $$status -ne 0 ]; then \
		printf '\n[ERROR] Command exited with status %s\n' "$$status" >> "$(UI_LOG)"; \
		$(UI) stop >/dev/null 2>&1 || true; \
		exit $$status; \
	fi
endef


# ============================================================
# Ensure UI exists
#
# A target can be invoked directly:
#
#     make kernel
#
# In that case there is no `all` target to start the UI first.
#
# UI_ENSURE checks whether the renderer is already alive.
# ============================================================

define UI_ENSURE
	@if [ ! -f "$(UI_PID)" ] || \
		! kill -0 "$$(cat "$(UI_PID)" 2>/dev/null)" 2>/dev/null; then \
		$(MAKE) --no-print-directory ui-start; \
	fi
endef


# ============================================================
# UI start target
# ============================================================

.PHONY: ui-start
ui-start:
	@chmod +x $(UI)
	@mkdir -p "$(UI_DIR)"
	@rm -f "$(UI_ACTIVE)" "$(UI_PID)"
	@touch "$(UI_ACTIVE)"
	@printf '%s|%s|%s\n' "0" "6" "Starting..." > "$(UI_STATE)"
	@$(UI) start 0 6 "Starting..." &
	@echo $$! > "$(UI_PID)"
	@sleep 0.12

# ============================================================
# UI stop target
# ============================================================

.PHONY: ui-stop
ui-stop:
	@$(UI) stop >/dev/null 2>&1 || true


# ============================================================
# Phony targets
# ============================================================

.PHONY: all
.PHONY: kernel
.PHONY: kernel-tests
.PHONY: user
.PHONY: iso
.PHONY: test-iso
.PHONY: limine
.PHONY: run
.PHONY: test
.PHONY: clean
.PHONY: cleaniso
.PHONY: rebuild


# ============================================================
# Default
# ============================================================

all: ui-start $(ISO) $(TEST_ISO)
	@chmod +x $(UI)
	@$(call UI_STAGE,6,6,Build complete)
	@sleep 0.25
	@$(UI) stop >/dev/null 2>&1 || true
	@sleep 0.15
	@printf '\n$(BOLD)$(GREEN)  ✓$(RESET) Build complete!\n'


# ============================================================
# Limine
# ============================================================

$(LIMINE):
	$(call UI_ENSURE)
	$(call UI_STAGE,1,6,Preparing Limine)

	@if [ ! -d "$(LIMINE_DIR)" ]; then \
		printf '%s\n' "Cloning Limine..." >> "$(UI_LOG)"; \
		git clone \
			--branch "$(LIMINE_BRANCH)" \
			--depth 1 \
			"$(LIMINE_REPO)" \
			"$(LIMINE_DIR)" >> "$(UI_LOG)" 2>&1; \
	fi

	$(call UI_RUN,Building Limine,$(MAKE) -C "$(LIMINE_DIR)")

	$(call UI_STAGE,1,6,Limine ready)


limine: $(LIMINE)


# ============================================================
# Kernel
# ============================================================

kernel:
	@chmod +x $(UI)
	$(call UI_ENSURE)
	$(call UI_STAGE,2,6,Building kernel)

	$(call UI_RUN,Compiling kernel,cargo build)

	$(call UI_STAGE,2,6,Kernel ready)


# ============================================================
# Test kernel
# ============================================================

kernel-tests:
	@chmod +x $(UI)
	$(call UI_ENSURE)
	$(call UI_STAGE,4,6,Building test kernel)

	$(call UI_RUN,Compiling test kernel,CARGO_TARGET_DIR="$(TEST_TARGET_DIR)" \
		cargo build --features test)

	$(call UI_STAGE,4,6,Test kernel ready)


# ============================================================
# Userspace
# ============================================================

user: $(USER_BIN)


$(USER_BIN): $(USER_SRC)
	$(call UI_ENSURE)
	$(call UI_STAGE,3,6,Building userspace)

	@mkdir -p "$(dir $(USER_BIN))"

	$(call UI_RUN,Assembling userspace,nasm \
		-f bin \
		"$(USER_SRC)" \
		-o "$(USER_BIN)")

	$(call UI_STAGE,3,6,Userspace ready)


# ============================================================
# Normal ISO
# ============================================================

$(ISO): kernel user $(LIMINE)
	@chmod +x $(UI)
	$(call UI_ENSURE)
	$(call UI_STAGE,5,6,Building normal ISO)

	$(call UI_RUN,Preparing ISO staging directory,rm -rf "$(ISO_ROOT)" && \
		mkdir -p "$(ISO_ROOT)/boot" && \
		mkdir -p "$(ISO_ROOT)/EFI/BOOT")

	$(call UI_RUN,Copying kernel,cp \
		"$(KERNEL)" \
		"$(ISO_ROOT)/boot/oxenna")

	$(call UI_RUN,Copying userspace,cp \
		"$(USER_BIN)" \
		"$(ISO_ROOT)/boot/user.bin")

	$(call UI_RUN,Copying Limine configuration,cp \
		"limine.conf" \
		"$(ISO_ROOT)/limine.conf")

	$(call UI_RUN,Copying BIOS boot image,cp \
		"$(LIMINE_DIR)/limine-bios-cd.bin" \
		"$(ISO_ROOT)/boot/")

	$(call UI_RUN,Copying UEFI boot image,cp \
		"$(LIMINE_DIR)/limine-uefi-cd.bin" \
		"$(ISO_ROOT)/boot/")

	$(call UI_RUN,Copying UEFI loader,cp \
		"$(LIMINE_DIR)/BOOTX64.EFI" \
		"$(ISO_ROOT)/EFI/BOOT/")

	$(call UI_RUN,Copying Limine BIOS file,cp \
		"$(LIMINE_DIR)/limine-bios.sys" \
		"$(ISO_ROOT)/boot/")

	$(call UI_RUN,Creating ISO,xorriso -as mkisofs \
		-R -r -J \
		-b boot/limine-bios-cd.bin \
		-no-emul-boot \
		-boot-load-size 4 \
		-boot-info-table \
		--efi-boot boot/limine-uefi-cd.bin \
		-efi-boot-part \
		--efi-boot-image \
		-o "$(ISO)" \
		"$(ISO_ROOT)")

	$(call UI_RUN,Installing Limine,$(LIMINE) \
		bios-install \
		"$(ISO)")

	$(call UI_STAGE,5,6,Normal ISO ready)


iso: $(ISO)


# ============================================================
# Test ISO
# ============================================================

$(TEST_ISO): kernel-tests $(LIMINE)
	@chmod +x $(UI)
	$(call UI_ENSURE)
	$(call UI_STAGE,6,6,Building test ISO)

	$(call UI_RUN,Preparing ISO staging directory,rm -rf "$(ISO_ROOT)" && \
		mkdir -p "$(ISO_ROOT)/boot" && \
		mkdir -p "$(ISO_ROOT)/EFI/BOOT")

	$(call UI_RUN,Copying test kernel,cp \
		"$(TEST_KERNEL)" \
		"$(ISO_ROOT)/boot/oxenna")

	$(call UI_RUN,Copying test Limine configuration,cp \
		"limine_test.conf" \
		"$(ISO_ROOT)/limine.conf")

	$(call UI_RUN,Copying BIOS boot image,cp \
		"$(LIMINE_DIR)/limine-bios-cd.bin" \
		"$(ISO_ROOT)/boot/")

	$(call UI_RUN,Copying UEFI boot image,cp \
		"$(LIMINE_DIR)/limine-uefi-cd.bin" \
		"$(ISO_ROOT)/boot/")

	$(call UI_RUN,Copying UEFI loader,cp \
		"$(LIMINE_DIR)/BOOTX64.EFI" \
		"$(ISO_ROOT)/EFI/BOOT/")

	$(call UI_RUN,Copying Limine BIOS file,cp \
		"$(LIMINE_DIR)/limine-bios.sys" \
		"$(ISO_ROOT)/boot/")

	$(call UI_RUN,Creating test ISO,xorriso -as mkisofs \
		-R -r -J \
		-b boot/limine-bios-cd.bin \
		-no-emul-boot \
		-boot-load-size 4 \
		-boot-info-table \
		--efi-boot boot/limine-uefi-cd.bin \
		-efi-boot-part \
		--efi-boot-image \
		-o "$(TEST_ISO)" \
		"$(ISO_ROOT)")

	$(call UI_RUN,Installing Limine,$(LIMINE) \
		bios-install \
		"$(TEST_ISO)")

	$(call UI_STAGE,6,6,Test ISO ready)


test-iso: $(TEST_ISO)


# ============================================================
# Run
# ============================================================

run: $(ISO)
	$(call UI_ENSURE)
	$(call UI_STAGE,1,1,Launching Oxenna)

	@$(UI) stop >/dev/null 2>&1 || true
	@sleep 0.15

	@qemu-system-x86_64 \
		-cdrom "$(ISO)" \
		-m 256M \
		-serial stdio \
		-monitor none \
		-device isa-debug-exit,iobase=0xf4,iosize=0x04


# ============================================================
# Tests
# ============================================================

test: $(TEST_ISO)
	$(call UI_ENSURE)
	$(call UI_STAGE,1,1,Preparing kernel tests)

	@$(UI) stop >/dev/null 2>&1 || true
	@sleep 0.15

	@printf '\n$(BOLD)$(CYAN)  RUNNING KERNEL TESTS$(RESET)\n'
	@printf '$(GRAY)  Kernel: $(TEST_KERNEL)$(RESET)\n'
	@printf '$(GRAY)  ISO:    $(TEST_ISO)$(RESET)\n\n'

	@set +e; \
	qemu-system-x86_64 \
		-cdrom "$(TEST_ISO)" \
		-m 256M \
		-serial stdio \
		-monitor none \
		-device isa-debug-exit,iobase=0xf4,iosize=0x04; \
	status=$$?; \
	printf '\n'; \
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
# Clean
# ============================================================

clean:
	$(call UI_ENSURE)
	$(call UI_STAGE,1,1,Cleaning build artifacts)

	$(call UI_RUN,Removing Cargo artifacts,cargo clean)

	$(call UI_RUN,Removing test target,rm -rf \
		"$(TEST_TARGET_DIR)")

	$(call UI_RUN,Removing ISO files,rm -f \
		"$(ISO)" \
		"$(TEST_ISO)")

	$(call UI_RUN,Removing ISO staging directory,rm -rf \
		"$(ISO_ROOT)")

	$(call UI_RUN,Removing userspace binary,rm -f \
		"$(USER_BIN)")

	$(call UI_STAGE,1,1,Clean complete)

	@$(UI) stop >/dev/null 2>&1 || true
	@sleep 0.15

	@printf '\n$(BOLD)$(GREEN)  ✓$(RESET) Clean complete!\n'


# ============================================================
# Clean ISOs
# ============================================================

cleaniso:
	$(call UI_ENSURE)
	$(call UI_STAGE,1,1,Cleaning ISO files)

	$(call UI_RUN,Removing ISO files,rm -f \
		"$(ISO)" \
		"$(TEST_ISO)")

	$(call UI_RUN,Removing ISO staging directory,rm -rf \
		"$(ISO_ROOT)")

	$(call UI_STAGE,1,1,ISO cleanup complete)

	@$(UI) stop >/dev/null 2>&1 || true
	@sleep 0.15

	@printf '\n$(BOLD)$(GREEN)  ✓$(RESET) ISO files removed\n'


# ============================================================
# Full rebuild
# ============================================================

rebuild:
	@$(MAKE) --no-print-directory clean
	@$(MAKE) --no-print-directory all


# ============================================================
# Cleanup on Ctrl-C
# ============================================================

.PHONY: ui-cleanup

ui-cleanup:
	@$(UI) stop >/dev/null 2>&1 || true


# ============================================================
# Misc
# ============================================================

.DELETE_ON_ERROR:

.DEFAULT_GOAL := all
