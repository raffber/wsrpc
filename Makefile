SHELL := /bin/bash
.ONESHELL:
.SHELLFLAGS := -eufo pipefail -c

curdir = $(shell dirname $(realpath $(lastword $(MAKEFILE_LIST))))
projdir = $(shell git rev-parse --show-toplevel)

ifeq ($(OS),Windows_NT)
	venv_exe_dir = $(curdir)/.venv/Scripts
	python = $(venv_exe_dir)/python.exe
	ruff = $(venv_exe_dir)/ruff.exe
	pyright = $(venv_exe_dir)/pyright.exe
else
	venv_exe_dir = $(curdir)/.venv/bin
	python = $(venv_exe_dir)/python
	ruff = $(venv_exe_dir)/ruff
	pyright = $(venv_exe_dir)/pyright
endif


.PHONY: help
help:
	@awk 'BEGIN {FS = ":.*##"; printf "\nUsage:\n  make \033[36m\033[0m\n"} /^[a-zA-Z_-]+:.*?##/ { printf "  \033[36m%-15s\033[0m %s\n", $$1, $$2 } /^##@/ { printf "\n\033[1m%s\033[0m\n", substr($$0, 5) } ' $(MAKEFILE_LIST)


python-lint: ## Run linters
	cd $(curdir)
	$(ruff) check python 


python-check: ## Run type check
	$(pyright) python


python-test: ## Run python tests
	cd $(curdir)
	$(python) -m pytest 


build-wheel: ## Build wheel
	cd $(curdir)
	rm -rf dist/
	uv build --wheel
