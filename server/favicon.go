package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"
)

const noneTTL = 14 * 24 * time.Hour

var faviconJobs sync.Map

func (s *Session) setCurrentCommand(cmd string) {
	s.mu.Lock()
	if s.cmd == cmd {
		s.mu.Unlock()
		return
	}
	s.cmd = cmd
	s.cmdActive = cmd != ""
	if cmd == "" {
		s.lastProc = ""
	}
	active := s.cmdActive
	s.mu.Unlock()

	state := map[string]string{"cmd": cmd}
	if cmd == "" {
		state["favicon"] = ""
		state["proc"] = ""
		s.sendState(state)
		return
	}

	s.sendState(state)
	s.resolveCommandIcon(cmd)
	if active {
		s.startProcPolling()
	}
}

func (s *Session) resolveCommandIcon(cmd string) {
	formula, ok := resolveFormula(cmd)
	if !ok {
		s.sendState(map[string]string{"favicon": ""})
		return
	}
	if filename, ok := cachedFavicon(formula); ok {
		s.sendState(map[string]string{"favicon": filename})
		return
	}
	if _, loaded := faviconJobs.LoadOrStore(formula, struct{}{}); loaded {
		return
	}
	go func() {
		defer faviconJobs.Delete(formula)
		filename, ok := fetchFavicon(formula, cmd)
		if !ok {
			return
		}
		s.mu.Lock()
		stillCurrent := s.cmd == cmd || s.lastProc == cmd
		s.mu.Unlock()
		if stillCurrent {
			s.sendState(map[string]string{"favicon": filename})
		}
	}()
}

func cachedFavicon(name string) (string, bool) {
	filename := safeIconName(name) + ".png"
	path := filepath.Join(faviconDir(), filename)
	if _, err := os.Stat(path); err == nil {
		return filename, true
	}
	none := filepath.Join(faviconDir(), safeIconName(name)+".none")
	if st, err := os.Stat(none); err == nil && time.Since(st.ModTime()) < noneTTL {
		return "", false
	}
	return "", false
}

func fetchFavicon(formula, cmd string) (string, bool) {
	name := safeIconName(formula)
	filename := name + ".png"
	out := filepath.Join(faviconDir(), filename)
	if _, err := os.Stat(out); err == nil {
		return filename, true
	}
	none := filepath.Join(faviconDir(), name+".none")
	if st, err := os.Stat(none); err == nil && time.Since(st.ModTime()) < noneTTL {
		return "", false
	}

	if runtime.GOOS == "darwin" {
		if ok := writeAppIcon(cmd, out); ok {
			return filename, true
		}
	}

	homepage, ok := brewHomepage(formula)
	if ok {
		if ok := fetchHomepageIcon(homepage, out); ok {
			return filename, true
		}
	}

	_ = os.WriteFile(none, []byte(time.Now().Format(time.RFC3339)), 0o644)
	return "", false
}

func resolveFormula(cmd string) (string, bool) {
	fields := strings.Fields(cmd)
	if len(fields) == 0 {
		return "", false
	}
	for len(fields) > 0 && isWrapper(fields[0]) {
		fields = fields[1:]
		if len(fields) > 0 && (fields[0] == "--" || strings.Contains(fields[0], "=")) {
			fields = fields[1:]
		}
	}
	if len(fields) == 0 {
		return "", false
	}
	if isShell(filepath.Base(fields[0])) {
		return "", false
	}
	for i := len(fields) - 1; i >= 0; i-- {
		if strings.Contains(fields[i], "/") {
			if f, ok := resolveBinaryFormula(filepath.Base(fields[i])); ok {
				return f, true
			}
			return strings.TrimSuffix(filepath.Base(fields[i]), filepath.Ext(fields[i])), true
		}
	}
	if f, ok := resolveBinaryFormula(filepath.Base(fields[0])); ok {
		return f, true
	}
	return filepath.Base(fields[0]), true
}

func isWrapper(s string) bool {
	switch filepath.Base(s) {
	case "sudo", "env", "time", "nice", "xargs", "doas":
		return true
	default:
		return false
	}
}

func isShell(s string) bool {
	switch s {
	case "zsh", "bash", "fish", "sh", "dash", "ksh", "tcsh":
		return true
	default:
		return false
	}
}

func resolveBinaryFormula(bin string) (string, bool) {
	for _, prefix := range []string{"/opt/homebrew/bin", "/usr/local/bin"} {
		target, err := os.Readlink(filepath.Join(prefix, bin))
		if err != nil {
			continue
		}
		if !filepath.IsAbs(target) {
			target = filepath.Join(prefix, target)
		}
		for _, marker := range []string{"/Cellar/", "/Caskroom/", "/node_modules/"} {
			idx := strings.Index(target, marker)
			if idx < 0 {
				continue
			}
			rest := target[idx+len(marker):]
			name := strings.Split(rest, string(os.PathSeparator))[0]
			if name != "" {
				return name, true
			}
		}
	}
	return "", false
}

func lookPath(bin string) (string, error) {
	for _, prefix := range []string{"/opt/homebrew/bin", "/usr/local/bin"} {
		p := filepath.Join(prefix, bin)
		if _, err := os.Stat(p); err == nil {
			return p, nil
		}
	}
	return exec.LookPath(bin)
}

func writeAppIcon(cmd, out string) bool {
	fields := strings.Fields(cmd)
	if len(fields) == 0 {
		return false
	}
	bin, err := lookPath(filepath.Base(fields[0]))
	if err != nil {
		return false
	}
	real, err := filepath.EvalSymlinks(bin)
	if err != nil {
		real = bin
	}
	app := findAppBundle(real)
	if app == "" {
		return false
	}
	icon, err := plistValue(filepath.Join(app, "Contents", "Info.plist"), "CFBundleIconFile")
	if err != nil || icon == "" {
		return false
	}
	if filepath.Ext(icon) == "" {
		icon += ".icns"
	}
	icns := filepath.Join(app, "Contents", "Resources", icon)
	if _, err := os.Stat(icns); err != nil {
		return false
	}
	cmdSips := exec.Command("sips", "-s", "format", "png", icns, "--out", out)
	return cmdSips.Run() == nil
}

func findAppBundle(path string) string {
	for dir := path; dir != "/" && dir != "."; dir = filepath.Dir(dir) {
		if strings.HasSuffix(dir, ".app") {
			return dir
		}
	}
	return ""
}

func plistValue(plist, key string) (string, error) {
	out, err := exec.Command("/usr/libexec/PlistBuddy", "-c", "Print :"+key, plist).Output()
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(string(out)), nil
}

func brewPath() string {
	for _, p := range []string{"/opt/homebrew/bin/brew", "/usr/local/bin/brew"} {
		if _, err := os.Stat(p); err == nil {
			return p
		}
	}
	return "brew"
}

func brewHomepage(formula string) (string, bool) {
	out, err := exec.Command(brewPath(), "info", "--json=v2", formula).Output()
	if err != nil {
		return "", false
	}
	var data struct {
		Formulae []struct {
			Homepage string `json:"homepage"`
		} `json:"formulae"`
		Casks []struct {
			Homepage string `json:"homepage"`
		} `json:"casks"`
	}
	if err := json.Unmarshal(out, &data); err != nil {
		return "", false
	}
	if len(data.Formulae) > 0 && data.Formulae[0].Homepage != "" {
		return data.Formulae[0].Homepage, true
	}
	if len(data.Casks) > 0 && data.Casks[0].Homepage != "" {
		return data.Casks[0].Homepage, true
	}
	return "", false
}

func fetchHomepageIcon(homepage, out string) bool {
	current := homepage
	seen := map[string]bool{}
	client := &http.Client{Timeout: 5 * time.Second}
	for depth := 0; depth < 8; depth++ {
		if seen[current] {
			return false
		}
		seen[current] = true
		resp, err := client.Get(current)
		if err != nil {
			return false
		}
		finalURL := resp.Request.URL.String()
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 8<<10))
		resp.Body.Close()
		current = finalURL
		if meta, ok := metaRefreshURL(body); ok {
			current = resolveURL(current, meta)
			continue
		}
		if isGitHubRepo(current) {
			gh, ok := resolveGitHubHomepage(current)
			if !ok {
				return false
			}
			current = gh
			if strings.Contains(current, "avatars.githubusercontent.com") {
				break
			}
			continue
		}
		break
	}
	if strings.Contains(current, "avatars.githubusercontent.com") {
		return downloadFile(current, out)
	}
	return downloadFile(fmt.Sprintf("https://www.google.com/s2/favicons?domain=%s&sz=64", url.QueryEscape(current)), out)
}

func metaRefreshURL(body []byte) (string, bool) {
	lower := strings.ToLower(string(body))
	idx := strings.Index(lower, "http-equiv")
	if idx < 0 || !strings.Contains(lower[idx:], "refresh") {
		return "", false
	}
	urlIdx := strings.Index(lower[idx:], "url=")
	if urlIdx < 0 {
		return "", false
	}
	start := idx + urlIdx + len("url=")
	raw := string(body[start:])
	raw = strings.TrimLeft(raw, " '\"")
	end := strings.IndexAny(raw, "'\">; \n\r\t")
	if end >= 0 {
		raw = raw[:end]
	}
	return strings.TrimSpace(raw), raw != ""
}

func resolveURL(base, ref string) string {
	b, err := url.Parse(base)
	if err != nil {
		return ref
	}
	r, err := url.Parse(ref)
	if err != nil {
		return ref
	}
	return b.ResolveReference(r).String()
}

func isGitHubRepo(raw string) bool {
	u, err := url.Parse(raw)
	if err != nil || u.Host != "github.com" {
		return false
	}
	parts := strings.Split(strings.Trim(u.Path, "/"), "/")
	return len(parts) >= 2 && parts[0] != "" && parts[1] != ""
}

func resolveGitHubHomepage(raw string) (string, bool) {
	u, err := url.Parse(raw)
	if err != nil {
		return "", false
	}
	parts := strings.Split(strings.Trim(u.Path, "/"), "/")
	if len(parts) < 2 {
		return "", false
	}
	api := "https://api.github.com/repos/" + parts[0] + "/" + parts[1]
	req, _ := http.NewRequest(http.MethodGet, api, nil)
	req.Header.Set("User-Agent", "tabsh")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", false
	}
	defer resp.Body.Close()
	var data struct {
		Homepage string `json:"homepage"`
		Owner    struct {
			AvatarURL string `json:"avatar_url"`
			Type      string `json:"type"`
		} `json:"owner"`
	}
	if err := json.NewDecoder(io.LimitReader(resp.Body, 1<<20)).Decode(&data); err != nil {
		return "", false
	}
	if data.Homepage != "" {
		return data.Homepage, true
	}
	if data.Owner.Type == "Organization" && data.Owner.AvatarURL != "" {
		return data.Owner.AvatarURL, true
	}
	return "", false
}

func downloadFile(raw, out string) bool {
	resp, err := http.Get(raw)
	if err != nil {
		return false
	}
	defer resp.Body.Close()
	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		return false
	}
	tmp := out + ".tmp"
	f, err := os.Create(tmp)
	if err != nil {
		return false
	}
	_, copyErr := io.Copy(f, io.LimitReader(resp.Body, 2<<20))
	closeErr := f.Close()
	if copyErr != nil || closeErr != nil {
		_ = os.Remove(tmp)
		return false
	}
	return os.Rename(tmp, out) == nil
}

func safeIconName(name string) string {
	name = strings.TrimSpace(strings.ToLower(name))
	var b strings.Builder
	for _, r := range name {
		if r >= 'a' && r <= 'z' || r >= '0' && r <= '9' || r == '-' || r == '_' || r == '.' {
			b.WriteRune(r)
		} else {
			b.WriteByte('_')
		}
	}
	if b.Len() == 0 {
		return "unknown"
	}
	return b.String()
}

var errUnsupportedProc = errors.New("unsupported process lookup")
