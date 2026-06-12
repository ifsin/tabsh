BINARY   := tabsh
GO_DIR   := server
HTML_DIR := client/web
WASM_DIR := client/wasm

.PHONY: all wasm frontend server clean

all: server

wasm:
	wasm-pack build $(WASM_DIR) --target web --out-dir ../../$(HTML_DIR)/src/wasm

frontend: wasm
	cd $(HTML_DIR) && npm run build

server: frontend
	rm -rf $(GO_DIR)/shims
	cp -R shims $(GO_DIR)/shims
	cd $(GO_DIR) && go build -o ../$(BINARY) .

clean:
	rm -rf $(HTML_DIR)/dist $(HTML_DIR)/src/wasm
	rm -rf $(GO_DIR)/embedded
	rm -rf $(GO_DIR)/shims
	cd $(GO_DIR) && go clean
	rm -f $(BINARY)
