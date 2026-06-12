package main

import (
	"crypto/sha256"
	"embed"
	"encoding/hex"
	"io/fs"
	"log"
	"os"
	"path/filepath"
)

//go:embed shims
var shimsFS embed.FS

var globalShimDir string

func getShimDir() string {
	return globalShimDir
}

func extractShims() {
	dir := filepath.Join(os.TempDir(), "tabsh-shims")
	globalShimDir = dir

	version, err := shimVersion()
	if err != nil {
		log.Printf("failed to hash embedded shims: %v", err)
		return
	}

	versionPath := filepath.Join(dir, ".version")
	if current, err := os.ReadFile(versionPath); err == nil && string(current) == version {
		return
	}

	if err := os.RemoveAll(dir); err != nil {
		log.Printf("failed to reset shim dir: %v", err)
		return
	}
	if err := os.MkdirAll(dir, 0o755); err != nil {
		log.Printf("failed to create shim dir: %v", err)
		return
	}

	err = fs.WalkDir(shimsFS, "shims", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		rel, err := filepath.Rel("shims", path)
		if err != nil {
			return err
		}
		if rel == "." {
			return nil
		}
		target := filepath.Join(dir, rel)
		if d.IsDir() {
			return os.MkdirAll(target, 0o755)
		}
		if filepath.Dir(rel) == filepath.Join("init", "zsh") {
			target = filepath.Join(dir, "init", "zsh", "."+filepath.Base(rel))
		}

		data, err := shimsFS.ReadFile(path)
		if err != nil {
			return err
		}

		return os.WriteFile(target, data, 0o755)
	})
	if err != nil {
		log.Printf("failed to extract shims: %v", err)
	} else {
		if err := os.WriteFile(versionPath, []byte(version), 0o644); err != nil {
			log.Printf("failed to write shim version: %v", err)
		}
	}
}

func shimVersion() (string, error) {
	h := sha256.New()
	err := fs.WalkDir(shimsFS, "shims", func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.IsDir() {
			return nil
		}
		data, err := shimsFS.ReadFile(path)
		if err != nil {
			return err
		}
		h.Write([]byte(path))
		h.Write([]byte{0})
		h.Write(data)
		h.Write([]byte{0})
		return nil
	})
	if err != nil {
		return "", err
	}
	return hex.EncodeToString(h.Sum(nil)), nil
}
