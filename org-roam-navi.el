;;; org-roam-navi.el --- The Navi graph, driven from Emacs -*- lexical-binding: t; -*-

;; Copyright (C) 2026 Matthew Gantenbein

;; Author: Matthew Gantenbein
;; URL: https://codeberg.org/ganten1998/navi
;; Package-Version: 1.2.0
;; Package-Requires: ((emacs "28.1") (org-roam "2.2"))
;; Keywords: outlines, hypermedia, convenience

;; This file is not part of GNU Emacs.

;;; Commentary:

;; Navi is a fast native graph viewer for org-roam.  This package makes it
;; part of Emacs:
;;
;;   M-x org-roam-navi    open the graph (downloads Navi the first time)
;;
;; and then, with no further setup:
;;
;; - the graph follows you: the note (or heading) at point is selected and
;;   the view glides to it, so with Navi's local-graph mode (L) you always
;;   see the neighbourhood of what you're writing;
;; - double-clicking a node (or Enter on it) opens it in this Emacs, in the
;;   window you were last in.  No `server-start', no emacsclient;
;; - Navi draws with your Emacs theme's colours, and follows theme changes;
;; - the graph re-reads the database as soon as org-roam writes it;
;; - Navi closes when Emacs does.
;;
;; Navi talks to Emacs over its stdin/stdout, so it works the same from GUI
;; frames, terminal frames (emacsclient -t) and on every OS Navi runs on
;; (macOS, Linux, Windows, FreeBSD).
;;
;; Install (Emacs 30):
;;
;;   (use-package org-roam-navi
;;     :vc (:url "https://codeberg.org/ganten1998/navi")
;;     :bind ("C-c n g" . org-roam-navi))
;;
;; See the README for Emacs 29, straight, elpaca and Doom.

;;; Code:

(require 'json)
(require 'seq)
(require 'subr-x)

(declare-function org-roam-id-at-point "org-roam-node")
(declare-function org-fold-show-context "org-fold")
(declare-function org-show-context "org")
(defvar org-roam-db-location)
(defvar url-http-end-of-headers)

(defgroup org-roam-navi nil
  "The Navi graph viewer, driven from Emacs."
  :group 'org-roam
  :prefix "org-roam-navi-"
  :link '(url-link "https://codeberg.org/ganten1998/navi"))

(defcustom org-roam-navi-program nil
  "The Navi executable.
nil means the copy `org-roam-navi-install' downloaded, then (on macOS)
Navi.app in /Applications or ~/Applications, offering to download Navi
when there is none.  On macOS, point this at Navi.app/Contents/MacOS/navi."
  :type '(choice (const :tag "Find or download" nil) file))

(defcustom org-roam-navi-follow t
  "Non-nil means the graph selects and glides to the node at point."
  :type 'boolean)

(defcustom org-roam-navi-follow-delay 0.25
  "Seconds of idle time before the graph follows point."
  :type 'number)

(defcustom org-roam-navi-sync-theme t
  "Non-nil means Navi draws with the current Emacs theme's colours.
Navi's own themes stay one press of T away."
  :type 'boolean)

(defcustom org-roam-navi-display-function #'pop-to-buffer-same-window
  "How a note opened from the graph is shown.
Called with the note's buffer, in the window that was selected last."
  :type '(choice (function-item pop-to-buffer-same-window)
                 (function-item pop-to-buffer)
                 (function-item switch-to-buffer-other-window)
                 function))

(defcustom org-roam-navi-install-directory
  (locate-user-emacs-file "org-roam-navi/")
  "Where `org-roam-navi-install' puts Navi."
  :type 'directory)

(defcustom org-roam-navi-release-url
  "https://codeberg.org/api/v1/repos/ganten1998/navi/releases/latest"
  "The API address of Navi's latest release."
  :type 'string)

(defvar org-roam-navi-open-hook nil
  "Run after a node opened from the graph is shown, in its buffer.")

(defconst org-roam-navi--protocol 1
  "The protocol version this file speaks; Navi says its own in `hello'.")

(defvar org-roam-navi--process nil)
(defvar org-roam-navi--partial "" "Output not yet ended by a newline.")
(defvar org-roam-navi--last-shown nil "The (ID . FILE) last sent with `show'.")
(defvar org-roam-navi--timer nil)
(defvar org-roam-navi--hello nil "Navi's `hello' event, once it has said it.")


;;;; Talking to Navi

(defun org-roam-navi--live-p ()
  "Non-nil when Navi is running under this Emacs."
  (process-live-p org-roam-navi--process))

(defun org-roam-navi--send (&rest fields)
  "Send Navi one command made of FIELDS, a plist like (:op \"show\" :id ID)."
  (when (org-roam-navi--live-p)
    (let (alist)
      (while fields
        (push (cons (substring (symbol-name (pop fields)) 1) (pop fields)) alist))
      (process-send-string
       org-roam-navi--process
       (concat (json-encode (nreverse alist)) "\n")))))

(defun org-roam-navi--parse (line)
  "LINE as a plist, or nil when it isn't a JSON object."
  (condition-case nil
      (if (fboundp 'json-parse-string)
          (json-parse-string line :object-type 'plist :null-object nil)
        (let ((json-object-type 'plist) (json-key-type 'keyword))
          (json-read-from-string line)))
    (error nil)))

(defun org-roam-navi--filter (_proc output)
  "Handle OUTPUT from Navi: one JSON event per line."
  (let ((lines (split-string (concat org-roam-navi--partial output) "\n")))
    (setq org-roam-navi--partial (car (last lines)))
    (dolist (line (butlast lines))
      (when-let* ((event (org-roam-navi--parse line)))
        (when (and (consp event) (keywordp (car event)))
          (org-roam-navi--handle event))))))

(defun org-roam-navi--handle (event)
  "Act on one EVENT from Navi."
  (pcase (plist-get event :event)
    ("hello"
     (setq org-roam-navi--hello event)
     (let ((theirs (or (plist-get event :protocol) 0)))
       (when (< theirs org-roam-navi--protocol)
         (display-warning
          'org-roam-navi
          (format "Navi %s is older than this package expects; M-x org-roam-navi-install updates it"
                  (plist-get event :version)))))
     (setq org-roam-navi--last-shown nil)
     (when org-roam-navi-sync-theme (org-roam-navi--send-theme))
     (org-roam-navi--follow-tick))
    ("open"
     (org-roam-navi--visit (plist-get event :file)
                           (plist-get event :pos)
                           (plist-get event :level)))))

(defun org-roam-navi--visit (file pos level)
  "Show FILE, at POS when LEVEL says it's a heading node."
  (if (not (and (stringp file) (file-exists-p file)))
      (message "Navi: no such file %s" file)
    (let ((buf (find-file-noselect file)))
      (funcall org-roam-navi-display-function buf)
      (when-let* ((win (get-buffer-window buf)))
        (with-selected-window win
          (when (and (integerp level) (> level 0) (integerp pos))
            (widen)
            (goto-char pos)
            (cond ((fboundp 'org-fold-show-context) (org-fold-show-context))
                  ((fboundp 'org-show-context) (org-show-context)))
            (recenter))
          (run-hooks 'org-roam-navi-open-hook)))
      ;; the graph showed this node already; don't send it straight back
      (with-current-buffer buf
        (setq org-roam-navi--last-shown (org-roam-navi--here)))
      (when (display-graphic-p)
        (select-frame-set-input-focus (selected-frame))))))

(defun org-roam-navi--sentinel (proc _event)
  "Tidy up when Navi (PROC) exits."
  (unless (process-live-p proc)
    (when (eq proc org-roam-navi--process)
      (org-roam-navi--teardown)
      (unless (memq (process-exit-status proc) '(0 9 15))
        (message "Navi exited (%s); see the *navi-log* buffer"
                 (process-exit-status proc))))))

(defun org-roam-navi--teardown ()
  "Forget the Navi process and stop following."
  (setq org-roam-navi--process nil
        org-roam-navi--hello nil
        org-roam-navi--partial ""
        org-roam-navi--last-shown nil)
  (when org-roam-navi--timer
    (cancel-timer org-roam-navi--timer)
    (setq org-roam-navi--timer nil))
  (remove-hook 'kill-emacs-hook #'org-roam-navi--on-kill-emacs)
  (when (boundp 'enable-theme-functions)
    (remove-hook 'enable-theme-functions #'org-roam-navi--theme-changed))
  (advice-remove 'enable-theme #'org-roam-navi--theme-changed))

(defun org-roam-navi--on-kill-emacs ()
  "Let Navi save its views before Emacs goes."
  (when (org-roam-navi--live-p)
    (ignore-errors
      (org-roam-navi--send :op "quit")
      (process-send-eof org-roam-navi--process)
      (let ((deadline (+ (float-time) 1.0)))
        (while (and (org-roam-navi--live-p) (< (float-time) deadline))
          (accept-process-output org-roam-navi--process 0.05))))))


;;;; Following point

(defun org-roam-navi--here ()
  "The (ID . FILE) of the node at point, or nil outside an org file."
  (when (and (derived-mode-p 'org-mode) (featurep 'org-roam))
    (let ((file (buffer-file-name (buffer-base-buffer)))
          (id (ignore-errors (org-roam-id-at-point))))
      (when (or id file)
        (cons id (and file (expand-file-name file)))))))

(defun org-roam-navi--follow-tick ()
  "Tell Navi about the node at point, when it changed."
  (when (and org-roam-navi-follow (org-roam-navi--live-p) org-roam-navi--hello)
    (when-let* ((here (with-current-buffer (window-buffer (selected-window))
                       (org-roam-navi--here))))
      (unless (equal here org-roam-navi--last-shown)
        (setq org-roam-navi--last-shown here)
        (org-roam-navi--send :op "show" :id (car here) :file (cdr here))))))


;;;; Theme

(defun org-roam-navi--colour (face attribute)
  "FACE's ATTRIBUTE as \"#rrggbb\", or nil when it has none."
  (when (facep face)
    (let ((c (face-attribute face attribute nil t)))
      (when (and (stringp c) (not (string-prefix-p "unspecified" c)))
        (when-let* ((v (color-values c)))
          (apply #'format "#%02x%02x%02x"
                 (mapcar (lambda (x) (/ x 257)) v)))))))

(defun org-roam-navi--send-theme ()
  "Send Navi the colours of the selected frame's faces."
  (org-roam-navi--send
   :op "theme"
   :bg (org-roam-navi--colour 'default :background)
   :fg (org-roam-navi--colour 'default :foreground)
   :dim (org-roam-navi--colour 'shadow :foreground)
   :link (or (org-roam-navi--colour 'org-link :foreground)
             (org-roam-navi--colour 'link :foreground))
   :accent (org-roam-navi--colour 'font-lock-keyword-face :foreground)
   :warm (org-roam-navi--colour 'warning :foreground)
   :bar_bg (org-roam-navi--colour 'mode-line :background)
   :bar_fg (org-roam-navi--colour 'mode-line :foreground)))

(defun org-roam-navi--theme-changed (&rest _)
  "A theme was enabled: send the new colours once faces settle."
  (when org-roam-navi-sync-theme
    (run-at-time 0.1 nil #'org-roam-navi--send-theme)))


;;;; Finding and installing Navi

(defun org-roam-navi--find-binary (dir)
  "The Navi executable unpacked under DIR, or nil."
  (when (file-directory-p dir)
    (if (eq system-type 'darwin)
        (let ((app (car (directory-files-recursively
                         dir "\\`Navi\\.app\\'" t
                         (lambda (d) (not (string-suffix-p ".app" d)))))))
          (when app
            (let ((bin (expand-file-name "Contents/MacOS/navi" app)))
              (and (file-executable-p bin) bin))))
      (seq-find #'file-executable-p
                (directory-files-recursively
                 dir (if (eq system-type 'windows-nt)
                         "\\`navi\\.exe\\'" "\\`navi\\'"))))))

(defun org-roam-navi--program ()
  "The Navi executable to run, or nil when there is none."
  (cond
   (org-roam-navi-program
    (let ((p (expand-file-name org-roam-navi-program)))
      (cond ((file-executable-p p) p)
            ((executable-find org-roam-navi-program))
            (t (user-error "`org-roam-navi-program' %s isn't an executable"
                           org-roam-navi-program)))))
   ((org-roam-navi--find-binary org-roam-navi-install-directory))
   ((eq system-type 'darwin)
    (seq-find #'file-executable-p
              (list "/Applications/Navi.app/Contents/MacOS/navi"
                    (expand-file-name "~/Applications/Navi.app/Contents/MacOS/navi"))))))

(defun org-roam-navi--asset-regexp ()
  "A regexp matching the release file for this machine."
  (let ((arch (car (split-string system-configuration "-"))))
    (pcase system-type
      ('darwin (if (member arch '("aarch64" "arm64"))
                   "\\`Navi-.*-\\(universal\\|arm64\\)\\.zip\\'"
                 "\\`Navi-.*-\\(universal\\|x86_64\\)\\.zip\\'"))
      ('windows-nt "\\`navi-.*-windows-x86_64\\.zip\\'")
      ('gnu/linux (format "\\`navi-.*-linux-%s\\.tar\\.gz\\'"
                          (if (member arch '("aarch64" "arm64")) "aarch64" "x86_64")))
      ('berkeley-unix
       (if (string-match-p "freebsd" system-configuration)
           "\\`navi-.*-freebsd-x86_64\\.tar\\.gz\\'"
         (user-error "Navi has no release for %s; build it from source" system-configuration)))
      (_ (user-error "Navi has no release for %s; build it from source" system-type)))))

(defun org-roam-navi--asset-p (name)
  "Non-nil when NAME is the release file for this machine."
  (let ((case-fold-search nil)) ; Navi-*.zip is macOS, navi-*.zip Windows
    (and (stringp name) (string-match-p (org-roam-navi--asset-regexp) name))))

(defun org-roam-navi--fetch-json (url)
  "GET URL and parse the JSON reply as a plist."
  (let ((buf (url-retrieve-synchronously url t t 30)))
    (unless buf (user-error "Navi: couldn't reach %s" url))
    (with-current-buffer buf
      (unwind-protect
          (progn
            (goto-char (point-min))
            (unless (re-search-forward "\\`HTTP/[0-9.]+ 200" (line-end-position) t)
              (user-error "Navi: %s answered %s" url
                          (buffer-substring (point-min) (line-end-position))))
            (goto-char (or url-http-end-of-headers (point-min)))
            (let ((json-object-type 'plist) (json-array-type 'list) (json-key-type 'keyword))
              (decode-coding-region (point) (point-max) 'utf-8)
              (json-read)))
        (kill-buffer buf)))))

(defun org-roam-navi--sha256 (file)
  "FILE's SHA-256 in hex."
  (with-temp-buffer
    (set-buffer-multibyte nil)
    (insert-file-contents-literally file)
    (secure-hash 'sha256 (current-buffer))))

(defun org-roam-navi--unpack (archive dir)
  "Unpack ARCHIVE into DIR with the tool this OS has."
  (make-directory dir t)
  (let* ((default-directory dir)
         (cmd (pcase system-type
                ('darwin (list "ditto" "-x" "-k" archive dir))
                ('windows-nt
                 ;; Windows 10+ ships bsdtar, which reads zips; an MSYS tar
                 ;; earlier on PATH doesn't
                 (list (expand-file-name "System32/tar.exe"
                                         (or (getenv "SystemRoot") "C:/Windows"))
                       "-xf" archive "-C" dir))
                (_ (list "tar" "-xzf" archive "-C" dir))))
         (status (with-temp-buffer
                   (list (apply #'call-process (car cmd) nil t nil (cdr cmd))
                         (buffer-string)))))
    (unless (eql (car status) 0)
      (error "Navi: unpacking %s failed: %s" archive (string-trim (cadr status))))))

;;;###autoload
(defun org-roam-navi-install ()
  "Download the latest Navi release for this machine.
It goes to `org-roam-navi-install-directory', replacing any older copy."
  (interactive)
  (require 'url)
  (message "Navi: looking up the latest release...")
  (let* ((release (org-roam-navi--fetch-json org-roam-navi-release-url))
         (tag (plist-get release :tag_name))
         (assets (plist-get release :assets))
         (asset (seq-find (lambda (a) (org-roam-navi--asset-p (plist-get a :name))) assets))
         (sums (seq-find (lambda (a) (equal (plist-get a :name) "SHA256SUMS")) assets))
         (dir (file-name-as-directory (expand-file-name org-roam-navi-install-directory)))
         (archive (expand-file-name (concat "download-" (plist-get asset :name))
                                    temporary-file-directory)))
    (unless asset
      (user-error "Navi %s has no download for this machine (%s)" tag system-configuration))
    (unless sums
      (user-error "Navi %s has no SHA256SUMS to check the download against; not installed" tag))
    (make-directory (file-name-directory (directory-file-name dir)) t)
    (message "Navi: downloading %s..." (plist-get asset :name))
    (url-copy-file (plist-get asset :browser_download_url) archive t)
    (let ((stage (make-temp-file (expand-file-name ".navi-new-" (file-name-directory (directory-file-name dir))) t))
          (sums-file (make-temp-file "navi-sums")))
      (unwind-protect
          (progn
            (url-copy-file (plist-get sums :browser_download_url) sums-file t)
            (let ((want (with-temp-buffer
                          (insert-file-contents sums-file)
                          (when (re-search-forward
                                 (concat "^\\([0-9a-f]\\{64\\}\\) [ *]?"
                                         (regexp-quote (plist-get asset :name)) "$")
                                 nil t)
                            (match-string 1)))))
              (unless want
                (error "Navi: SHA256SUMS doesn't list %s; not installed" (plist-get asset :name)))
              (unless (equal want (org-roam-navi--sha256 archive))
                (error "Navi: %s doesn't match its SHA256SUMS; not installed"
                       (plist-get asset :name))))
            ;; unpack beside the old copy and only then swap, so a failure
            ;; anywhere leaves the working Navi in place
            (org-roam-navi--unpack archive stage)
            (unless (org-roam-navi--find-binary stage)
              (error "Navi: no navi executable in %s" (plist-get asset :name)))
            (with-temp-file (expand-file-name "VERSION" stage) (insert tag "\n"))
            (when (org-roam-navi--live-p)
              (org-roam-navi-quit)
              (let ((deadline (+ (float-time) 5)))
                (while (and (org-roam-navi--live-p) (< (float-time) deadline))
                  (accept-process-output nil 0.05))))
            (when (file-directory-p dir)
              ;; never wipe a directory this package didn't fill
              (unless (or (file-exists-p (expand-file-name "VERSION" dir))
                          (null (directory-files dir nil directory-files-no-dot-files-regexp)))
                (error "Navi: %s holds files this package didn't put there; set `org-roam-navi-install-directory' to an empty directory" dir))
              (delete-directory dir t))
            (rename-file stage (directory-file-name dir)))
        (when (file-directory-p stage) (delete-directory stage t))
        (delete-file sums-file)
        (delete-file archive)))
    (message "Navi %s installed in %s" tag dir)
    tag))

;;;###autoload
(defun org-roam-navi-update ()
  "Install the latest Navi when it is newer than the downloaded one."
  (interactive)
  (require 'url)
  (let* ((file (expand-file-name "VERSION" org-roam-navi-install-directory))
         (have (and (file-exists-p file)
                    (string-trim (with-temp-buffer (insert-file-contents file) (buffer-string)))))
         (latest (plist-get (org-roam-navi--fetch-json org-roam-navi-release-url) :tag_name)))
    (if (equal have latest)
        (message "Navi %s is the latest" have)
      (org-roam-navi-install))))


;;;; Commands

(defun org-roam-navi--start ()
  "Start Navi under this Emacs."
  (require 'org-roam)
  (let ((program (or (org-roam-navi--program)
                     (if (y-or-n-p "Navi isn't installed.  Download it from Codeberg (about 10 MB)? ")
                         (progn (org-roam-navi-install)
                                (org-roam-navi--find-binary org-roam-navi-install-directory))
                       (user-error "Navi isn't installed (M-x org-roam-navi-install)"))))
        (log (get-buffer-create "*navi-log*")))
    (with-current-buffer log
      (goto-char (point-max))
      (insert (format "\n--- %s %s\n" (format-time-string "%F %T") program)))
    (setq org-roam-navi--partial ""
          org-roam-navi--hello nil
          org-roam-navi--last-shown nil)
    (setq org-roam-navi--process
          (make-process
           :name "navi"
           :command (list program "--emacs" "--db" (expand-file-name org-roam-db-location))
           :connection-type 'pipe
           :coding 'utf-8-unix
           :noquery t
           :stderr log
           :filter #'org-roam-navi--filter
           :sentinel #'org-roam-navi--sentinel))
    (setq org-roam-navi--timer
          (run-with-idle-timer org-roam-navi-follow-delay t #'org-roam-navi--follow-tick))
    (add-hook 'kill-emacs-hook #'org-roam-navi--on-kill-emacs)
    (if (boundp 'enable-theme-functions)
        (add-hook 'enable-theme-functions #'org-roam-navi--theme-changed)
      ;; Emacs 28 has no hook for it
      (advice-add 'enable-theme :after #'org-roam-navi--theme-changed))
    (let ((proc org-roam-navi--process))
      (run-at-time 10 nil
                   (lambda ()
                     (when (and (eq proc org-roam-navi--process)
                                (process-live-p proc)
                                (not org-roam-navi--hello))
                       (display-warning
                        'org-roam-navi
                        (format "%s opened but doesn't talk to Emacs: it's older than Navi 1.2.  M-x org-roam-navi-install gets a current one"
                                program))))))))

;;;###autoload
(defun org-roam-navi ()
  "Open the Navi graph of your org-roam notes, or bring it to the front.
The graph then follows the node at point, and nodes opened in it open
here.  The first run offers to download Navi."
  (interactive)
  (if (org-roam-navi--live-p)
      (progn
        (setq org-roam-navi--last-shown nil)
        (org-roam-navi--follow-tick)
        (org-roam-navi--send :op "raise"))
    (org-roam-navi--start)))

;;;###autoload
(defun org-roam-navi-quit ()
  "Close the Navi graph."
  (interactive)
  (when (org-roam-navi--live-p)
    (org-roam-navi--send :op "quit")
    (process-send-eof org-roam-navi--process)))

(defun org-roam-navi-reload ()
  "Make Navi re-read the org-roam database now.
Navi already re-reads it whenever org-roam writes it."
  (interactive)
  (org-roam-navi--send :op "reload"))

(provide 'org-roam-navi)
;;; org-roam-navi.el ends here
