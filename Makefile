# FLTS Tauri app — build and install helpers.
#
# Usage:
#   make                  # build macOS desktop app (default)
#   make dev              # run Tauri dev server (hot reload)
#   make build-ios        # build iOS IPA (Release)
#   make build-android    # build signed Android APK
#   make install-macos    # build + copy to /Applications
#   make install-ios      # build Release + install on connected iPhone/iPad
#   make install-android  # build + adb install on connected device
#   make install-android-native  # the native Compose app instead (see docs/native-android.md)
#
# Spaced aliases (quoted on the shell): make "install macos" | "install ios" | "install android"
#
# Optional overrides:
#   IOS_DEVICE=<name|udid>   pick a specific iOS device (default: first booted device)
#   FLTS_KEYSTORE=...        Android release keystore (see site/build-android.sh)

ROOT := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
SITE := $(ROOT)site
TAURI_DIR := $(SITE)/src-tauri
PRODUCT := FLTS
MACOS_PROFILE := release-ship

CARGO_TARGET_DIR := $(shell cd "$(SITE)" && cargo metadata --format-version 1 --no-deps 2>/dev/null | python3 -c "import json,sys; print(json.load(sys.stdin)['target_directory'])")
MACOS_APP := $(CARGO_TARGET_DIR)/$(MACOS_PROFILE)/bundle/macos/$(PRODUCT).app
IOS_IPA := $(TAURI_DIR)/gen/apple/build/arm64/$(PRODUCT).ipa

.DEFAULT_GOAL := build

.PHONY: all build dev deps check-deps hooks format \
	build-macos build-ios build-android \
	install-macos install-ios install-android \
	build-ios-native install-ios-native \
	build-android-native install-android-native test-android-native test-android-native-ui \
	build-macos-native install-macos-native test-macos-native test-macos-native-ui test-macos-native-ui-quick test-macos-native-ui-translate test-ios-native-ui test-ios-native-ui-services \
	install\ macos install\ ios install\ android \
	help

all: build ## build the macOS desktop app (default)

help: ## show available targets
	@grep -E '^[a-zA-Z0-9 /\\_-]+:.*##' $(MAKEFILE_LIST) | sed 's/\\ / /g' | awk 'BEGIN {FS = ":.*## "}; {printf "  %-22s %s\n", $$1, $$2}'

deps: ## install frontend dependencies (pnpm, when missing)
	@if [ ! -d "$(SITE)/node_modules" ]; then cd "$(SITE)" && pnpm install; fi
	@$(MAKE) --no-print-directory check-deps

check-deps: ## verify Tauri crate and npm versions are on the same major/minor
	@"$(ROOT)scripts/check-tauri-versions.sh"

hooks: ## install git pre-commit formatters (requires pre-commit on PATH)
	"$(ROOT)scripts/install-git-hooks.sh"

format: ## apply all repo formatters (same as the pre-commit hook)
	@command -v pre-commit >/dev/null 2>&1 || { echo "pre-commit is required. Run: brew install pre-commit && make hooks" >&2; exit 1; }
	cd "$(ROOT)" && pre-commit run --all-files

dev: deps ## run Tauri in development mode (hot reload)
	cd "$(SITE)" && cargo tauri dev

build: build-macos ## build the Tauri app for the current dev platform (macOS)

build-macos: deps ## build macOS .app bundle (release-ship profile)
	cd "$(SITE)" && cargo tauri build -- --profile $(MACOS_PROFILE)

NATIVE := native
NATIVE_APP := $(NATIVE)/build/DerivedData/Build/Products/Release/$(PRODUCT).app

build-macos-native: ## build the native SwiftUI macOS app (Release, core in release-ship)
	cd "$(NATIVE)" && xcodegen generate && xcodebuild -project FLTS.xcodeproj -scheme FLTS \
		-configuration Release -derivedDataPath build/DerivedData build

test-macos-native: ## run the native app's unit tests
	cd "$(NATIVE)" && xcodegen generate && scripts/keep-prefs.sh xcodebuild -project FLTS.xcodeproj -scheme FLTS \
		-configuration Debug -derivedDataPath build/DerivedData test -only-testing:FLTSTests

test-macos-native-ui: ## run the full native UI tour on a fresh seeded library (needs an unlocked screen)
	"$(NATIVE)/scripts/ui-tests.sh"

test-macos-native-ui-quick: ## the core library and reader flow only, for routine runs
	QUICK=1 "$(NATIVE)/scripts/ui-tests.sh"

test-macos-native-ui-translate: ## on screen: translate and Anki against the e2e sims, sync with the engine kept local
	"$(NATIVE)/scripts/ui-translate.sh"

test-ios-native-ui: ## run the iOS UI tour in a Simulator, headless (SIM=<simulator id> NAME=<label> to pick one)
	"$(NATIVE)/scripts/ios-ui-tests.sh" $(SIM) $(NAME)

test-ios-native-ui-services: ## in a Simulator: translate and Anki against the e2e sims, library actions, local sync
	"$(NATIVE)/scripts/ios-ui-services.sh" $(SIM) $(NAME)

install-macos-native: build-macos-native ## replace /Applications/FLTS.app with the native app
	@if pgrep -xq "$(PRODUCT)"; then echo "quit $(PRODUCT) first: both apps share one library"; exit 1; fi
	rm -rf "/Applications/$(PRODUCT).app"
	ditto "$(NATIVE_APP)" "/Applications/$(PRODUCT).app"
	@echo ">> installed native /Applications/$(PRODUCT).app"

build-ios: deps ## build iOS IPA (Release; development export for device install)
	cd "$(SITE)" && cargo tauri ios build --export-method debugging --ci

build-android: deps ## build signed universal Android APK
	cd "$(SITE)" && ./build-android.sh

install-macos: build-macos ## install macOS app into /Applications
	@test -d "$(MACOS_APP)" || { echo "missing bundle: $(MACOS_APP)"; exit 1; }
	ditto "$(MACOS_APP)" "/Applications/$(PRODUCT).app"
	@echo ">> installed /Applications/$(PRODUCT).app"

install-ios: build-ios ## install on a connected iPhone/iPad
	@set -euo pipefail; \
	APP=""; \
	if [ -d "$(TAURI_DIR)/gen/apple/build/arm64/$(PRODUCT).app" ]; then \
		APP="$(TAURI_DIR)/gen/apple/build/arm64/$(PRODUCT).app"; \
	elif [ -f "$(IOS_IPA)" ]; then \
		EXTRACT="$$(mktemp -d)"; \
		trap 'rm -rf "$$EXTRACT"' EXIT; \
		unzip -q "$(IOS_IPA)" -d "$$EXTRACT"; \
		APP="$$(ls -d "$$EXTRACT"/Payload/*.app | head -1)"; \
	else \
		echo "missing iOS artifact (expected $(IOS_IPA))"; exit 1; \
	fi; \
	if [ -n "$${IOS_DEVICE:-}" ]; then \
		DEVICE="$$IOS_DEVICE"; \
	else \
		DEVICE="$$(xcrun devicectl list devices --json-output - 2>/dev/null | python3 -c 'import json,sys; data=json.load(sys.stdin); devices=data.get("result",{}).get("devices",[]); booted=[d for d in devices if d.get("deviceProperties",{}).get("bootState")=="booted"]; pool=booted or devices; (not pool) and sys.exit("no connected iOS device found (pair in Xcode > Devices and Simulators)"); d=pool[0]; print(d.get("deviceProperties",{}).get("name") or d.get("identifier"))')"; \
	fi; \
	echo ">> installing on $$DEVICE"; \
	xcrun devicectl device install app --device "$$DEVICE" "$$APP"

NATIVE_IOS_APP := $(NATIVE)/build/DerivedData-iOS-device/Build/Products/Release-iphoneos/$(PRODUCT).app

ANDROID_NATIVE := $(ROOT)android
ANDROID_NATIVE_APK := $(ANDROID_NATIVE)/app/build/outputs/apk/release/app-release.apk

build-android-native: ## build the native Compose Android app (release APK, core in release-ship, signed like build-android)
	cd "$(ANDROID_NATIVE)" && ./gradlew --console=plain :app:assembleRelease
	@echo ">> $(ANDROID_NATIVE_APK)"

install-android-native: build-android-native ## install the native app on a connected device (replaces the Tauri app, keeps its data)
	adb install -r "$(ANDROID_NATIVE_APK)"

test-android-native: ## run the native Android app's unit tests (JVM, no device)
	cd "$(ANDROID_NATIVE)" && FLTS_SKIP_CORE=1 ./gradlew --console=plain :app:testDebugUnitTest

test-android-native-ui: ## run the Android UI tour on a connected device or emulator, on a seeded library
	"$(ANDROID_NATIVE)/scripts/ui-tests.sh"

build-ios-native: ## build the native SwiftUI iOS app for devices (Release, signed with the project's team)
	cd "$(NATIVE)" && xcodegen generate && xcodebuild -project FLTS.xcodeproj -scheme FLTS-iOS \
		-configuration Release -destination 'generic/platform=iOS' \
		-derivedDataPath build/DerivedData-iOS-device -allowProvisioningUpdates build

install-ios-native: build-ios-native ## install the native app on a connected iPhone/iPad (replaces the Tauri app, keeps its data)
	@set -euo pipefail; \
	test -d "$(NATIVE_IOS_APP)" || { echo "missing app: $(NATIVE_IOS_APP)"; exit 1; }; \
	if [ -n "$${IOS_DEVICE:-}" ]; then \
		DEVICE="$$IOS_DEVICE"; \
	else \
		DEVICE="$$(xcrun devicectl list devices --json-output - 2>/dev/null | python3 -c 'import json,sys; data=json.load(sys.stdin); devices=data.get("result",{}).get("devices",[]); booted=[d for d in devices if d.get("deviceProperties",{}).get("bootState")=="booted"]; pool=booted or devices; (not pool) and sys.exit("no connected iOS device found (pair in Xcode > Devices and Simulators)"); d=pool[0]; print(d.get("deviceProperties",{}).get("name") or d.get("identifier"))')"; \
	fi; \
	echo ">> installing the native app on $$DEVICE"; \
	xcrun devicectl device install app --device "$$DEVICE" "$(NATIVE_IOS_APP)"

install-android: ## build + adb install on a connected Android device
	cd "$(SITE)" && ./build-android.sh --install

# Spaced target aliases (invoke as: make "install macos")
install\ macos: install-macos
install\ ios: install-ios
install\ android: install-android
