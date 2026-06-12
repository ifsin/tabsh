package main

import (
	"bytes"
	"embed"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"log"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
)

//go:embed embedded
var embeddedFS embed.FS

type ThemeConfig struct {
	Background  string  `json:"background"`
	Foreground  string  `json:"foreground"`
	Cursor      string  `json:"cursor"`
	CursorStyle string  `json:"cursor_style"`
	CursorBlink bool    `json:"cursor_blink"`
	FontSize    float64 `json:"font_size"`
	FontFamily  string  `json:"font_family"`
	LineHeight  float64 `json:"line_height"`
}

func defaultTheme() ThemeConfig {
	return ThemeConfig{
		Background:  "#1E1E1E",
		Foreground:  "#D4D4D4",
		Cursor:      "#AEAFAD",
		CursorStyle: "block",
		CursorBlink: false,
		FontSize:    13,
		FontFamily:  "Consolas,Liberation Mono,Menlo,Courier,monospace",
		LineHeight:  1.2,
	}
}

type ServerConfig struct {
	Port         int    `json:"port"`
	MaxClients   int    `json:"max_clients"`
	Signal       string `json:"signal"`
	TerminalType string `json:"terminal_type"`
	Debug        int    `json:"debug"`
}

func defaultServer() ServerConfig {
	return ServerConfig{
		Port:         7681,
		MaxClients:   0,
		Signal:       "SIGHUP",
		TerminalType: "xterm-256color",
		Debug:        0,
	}
}

type AppConfig struct {
	ID      string            `json:"id"`
	Name    string            `json:"name"`
	Command string            `json:"command"`
	Args    []string          `json:"args"`
	Cwd     string            `json:"cwd"`
	Env     map[string]string `json:"env"`
	Icon    string            `json:"icon"`
}

type Config struct {
	Apps   []AppConfig  `json:"apps"`
	Theme  ThemeConfig  `json:"theme"`
	Server ServerConfig `json:"server"`
}

var (
	flagPort       = flag.Int("port", 7681, "port to listen on")
	flagStaticDir  = flag.String("static", "", "directory to serve static files from")
	flagConfigPath = flag.String("config", defaultConfigPath(), "path to config.json")
	flagBind       = flag.String("bind", "127.0.0.1", "address to bind to")
)

func parseSignal(name string) syscall.Signal {
	switch strings.ToUpper(strings.TrimPrefix(strings.ToUpper(name), "SIG")) {
	case "HUP":
		return syscall.SIGHUP
	case "INT":
		return syscall.SIGINT
	case "TERM":
		return syscall.SIGTERM
	case "KILL":
		return syscall.SIGKILL
	case "QUIT":
		return syscall.SIGQUIT
	default:
		return syscall.SIGHUP
	}
}

func defaultConfigPath() string {
	home, _ := os.UserHomeDir()
	return home + "/.config/tabsh/config.json"
}

func loadConfig(path string) *Config {
	defaults := &Config{Theme: defaultTheme(), Server: defaultServer()}

	data, err := os.ReadFile(path)
	if err != nil {
		if os.IsNotExist(err) {
			writeDefaultConfig(path, defaults)
		}
		return defaults
	}
	if len(bytes.TrimSpace(data)) == 0 {
		writeDefaultConfig(path, defaults)
		return defaults
	}

	cfg := &Config{Theme: defaultTheme(), Server: defaultServer()}
	if err := json.Unmarshal(data, cfg); err != nil {
		log.Printf("config parse error: %v", err)
		return defaults
	}
	return cfg
}

func writeDefaultConfig(path string, cfg *Config) {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return
	}
	data, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		return
	}
	if err := os.WriteFile(path, append(data, '\n'), 0o644); err != nil {
		log.Printf("could not write default config: %v", err)
	}
}

func spaHandler(fileSystem http.FileSystem, fsys fs.FS, configScript string) http.Handler {
	fileServer := http.FileServer(fileSystem)
	serveIndex := func(w http.ResponseWriter, r *http.Request) {
		f, err := fileSystem.Open("/index.html")
		if err != nil {
			http.NotFound(w, r)
			return
		}
		defer f.Close()
		data, err := io.ReadAll(f)
		if err != nil {
			http.Error(w, "read error", http.StatusInternalServerError)
			return
		}
		injected := bytes.Replace(data, []byte("</head>"), []byte(configScript+"</head>"), 1)
		w.Header().Set("Content-Type", "text/html; charset=utf-8")
		w.Write(injected)
	}
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var exists bool
		if fsys != nil {
			_, err := fsys.Open(r.URL.Path[1:])
			exists = err == nil
		} else {
			f, err := fileSystem.Open(r.URL.Path)
			if err == nil {
				f.Close()
			}
			exists = err == nil
		}
		if !exists {
			if strings.HasPrefix(r.URL.Path, "/_") {
				http.NotFound(w, r)
				return
			}
			serveIndex(w, r)
			return
		}
		if r.URL.Path == "/" || r.URL.Path == "/index.html" {
			serveIndex(w, r)
			return
		}
		fileServer.ServeHTTP(w, r)
	})
}

func faviconDir() string {
	dir := filepath.Join(os.TempDir(), "tabsh-favicons")
	os.MkdirAll(dir, 0o755)
	return dir
}

func faviconHandler(w http.ResponseWriter, r *http.Request) {
	name := filepath.Base(r.PathValue("name"))
	if name == "." || name == ".." || name == "" {
		http.NotFound(w, r)
		return
	}

	switch r.Method {
	case http.MethodGet, http.MethodHead:
		if name != "default.ico" {
			path := filepath.Join(faviconDir(), name)
			if _, err := os.Stat(path); err == nil {
				http.ServeFile(w, r, path)
				return
			}
		}
		data, err := embeddedFS.ReadFile("embedded/default.ico")
		if err != nil {
			http.NotFound(w, r)
			return
		}
		w.Header().Set("Content-Type", http.DetectContentType(data))
		w.Write(data)

	case http.MethodPost, http.MethodPut:
		if name == "default.ico" {
			http.Error(w, "default.ico is read-only", http.StatusForbidden)
			return
		}
		body, err := io.ReadAll(io.LimitReader(r.Body, 1<<20))
		if err != nil || len(body) == 0 {
			http.Error(w, "bad body", http.StatusBadRequest)
			return
		}
		if err := os.WriteFile(filepath.Join(faviconDir(), name), body, 0o644); err != nil {
			http.Error(w, "write failed", http.StatusInternalServerError)
			return
		}
		w.WriteHeader(http.StatusNoContent)

	default:
		http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
	}
}

func main() {
	flag.Parse()
	flagPortExplicit := false
	flag.CommandLine.Visit(func(f *flag.Flag) {
		if f.Name == "port" {
			flagPortExplicit = true
		}
	})

	extractShims()

	cfg := loadConfig(*flagConfigPath)

	port := cfg.Server.Port
	if flagPortExplicit {
		port = *flagPort
	}

	if cfg.Server.Debug > 0 {
		log.SetFlags(log.LstdFlags | log.Lshortfile)
	}

	cfgJSON, _ := json.Marshal(cfg.Theme)
	configScript := `<script>window.__TABSH_CONFIG__=` + string(cfgJSON) + `;</script>`

	mux := http.NewServeMux()
	mux.HandleFunc("/_ws", makeWSHandler(cfg))
	mux.HandleFunc("/_fav/{name}", faviconHandler)

	if *flagStaticDir != "" {
		mux.Handle("/", spaHandler(http.Dir(*flagStaticDir), nil, configScript))
	} else {
		sub, err := fs.Sub(embeddedFS, "embedded")
		if err != nil {
			log.Fatal(err)
		}
		mux.Handle("/", spaHandler(http.FS(sub), sub, configScript))
	}

	addr := fmt.Sprintf("%s:%d", *flagBind, port)
	srv := &http.Server{Addr: addr, Handler: mux}

	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM)
	go func() {
		<-sig
		log.Println("shutting down")
		killAllSessions()
		os.Exit(0)
	}()

	log.Printf("tabsh listening on %s", addr)
	if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
		log.Fatal(err)
	}
}
