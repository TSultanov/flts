# FLTS make targets.

.PHONY: test-sync-docker
# Multi-node native-sync integration test (Docker). Builds the harness image,
# brings up three isolated nodes, and asserts two-node sync + roster mesh
# device introduction over real device-to-device connections.
test-sync-docker:
	./tests/docker/run.sh
