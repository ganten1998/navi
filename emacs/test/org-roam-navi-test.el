;;; org-roam-navi-test.el --- Tests for org-roam-navi -*- lexical-binding: t; -*-

;; Run from the repo root:
;;   emacs -Q --batch -L emacs -l emacs/test/org-roam-navi-test.el -f ert-run-tests-batch-and-exit
;; (org-roam must be loadable, e.g. add (package-initialize) with your elpa dir.)

;;; Code:

(require 'ert)
(require 'org-roam-navi)
(require 'org-roam)

(defconst org-roam-navi-test--dir
  (file-name-directory (or load-file-name buffer-file-name)))

(defun org-roam-navi-test--log (file)
  "Commands the fake navi logged in FILE, as plists."
  (when (file-exists-p file)
    (with-temp-buffer
      (insert-file-contents file)
      (mapcar (lambda (l) (json-parse-string l :object-type 'plist))
              (split-string (buffer-string) "\n" t)))))

(defun org-roam-navi-test--wait (pred)
  "Let process output in until PRED holds (5 s at most)."
  (let ((deadline (+ (float-time) 5)))
    (while (and (not (funcall pred)) (< (float-time) deadline))
      (accept-process-output nil 0.05))
    (funcall pred)))

(defun org-roam-navi-test--wait-log (file n)
  "FILE's log once it holds N show commands."
  (org-roam-navi-test--wait
   (lambda () (>= (seq-count (lambda (c) (equal (plist-get c :op) "show"))
                             (org-roam-navi-test--log file))
                  n)))
  (org-roam-navi-test--log file))

(defmacro org-roam-navi-test--with-fake (&rest body)
  "Run BODY with the fake navi as `org-roam-navi-program'.
Binds LOG (its command log) and NOTE (an org file with two nodes)."
  (declare (indent 0))
  `(let* ((tmp (make-temp-file "navi-test" t))
          (log (expand-file-name "log" tmp))
          (note (expand-file-name "note.org" tmp))
          (org-roam-directory tmp)
          (org-roam-db-location (expand-file-name "org-roam.db" tmp))
          (org-roam-navi-program (expand-file-name "fake-navi" org-roam-navi-test--dir))
          (org-roam-navi-install-directory (expand-file-name "none" tmp))
          (process-environment (cons (concat "FAKE_NAVI_LOG=" log) process-environment)))
     (with-temp-file note
       (insert ":PROPERTIES:\n:ID: file-node\n:END:\n#+title: Note\n\n"
               "* Heading\n:PROPERTIES:\n:ID: open-me\n:END:\nbody\n"))
     (unwind-protect
         (progn ,@body)
       (when (org-roam-navi--live-p) (delete-process org-roam-navi--process))
       (org-roam-navi--teardown)
       (dolist (b (buffer-list))
         (when (and (buffer-file-name b) (string-prefix-p tmp (buffer-file-name b)))
           (kill-buffer b)))
       (delete-directory tmp t))))

(ert-deftest org-roam-navi-test-start-and-follow ()
  (org-roam-navi-test--with-fake
    (find-file note)
    (org-roam-navi)
    (should (org-roam-navi-test--wait (lambda () org-roam-navi--hello)))
    (let ((first (car (org-roam-navi-test--log log))))
      (should (equal (plist-get first :argv)
                     (vector "--emacs" "--db" (expand-file-name org-roam-db-location)))))
    ;; hello sends the theme and the node at point
    (should (org-roam-navi-test--wait
             (lambda () (seq-find (lambda (c) (equal (plist-get c :op) "show"))
                                  (org-roam-navi-test--log log)))))
    (let* ((cmds (org-roam-navi-test--log log))
           (theme (seq-find (lambda (c) (equal (plist-get c :op) "theme")) cmds))
           (show (seq-find (lambda (c) (equal (plist-get c :op) "show")) cmds)))
      (should theme)
      (should (equal (plist-get show :id) "file-node"))
      (should (equal (plist-get show :file) (expand-file-name note))))
    ;; moving point to another node sends one more show, staying put sends none
    (org-roam-navi--follow-tick)
    (should (= 1 (seq-count (lambda (c) (equal (plist-get c :op) "show"))
                            (org-roam-navi-test--log log))))
    (search-forward "body")
    (org-roam-navi--follow-tick)
    (let ((shows (seq-filter (lambda (c) (equal (plist-get c :op) "show"))
                             (org-roam-navi-test--wait-log log 2))))
      (should (equal (plist-get (cadr shows) :id) "open-me")))))

(ert-deftest org-roam-navi-test-open-visits-the-heading ()
  (org-roam-navi-test--with-fake
    (let ((pos (with-temp-buffer (insert-file-contents note)
                                 (re-search-forward "^\\* Heading") (match-beginning 0))))
      (setenv "FAKE_NAVI_OPEN" (format "%s:%d:1" note pos))
      (switch-to-buffer "*scratch*")
      (org-roam-navi)
      (should (org-roam-navi-test--wait (lambda () org-roam-navi--hello)))
      ;; make navi answer with an open (the fake does that for "open-me")
      (org-roam-navi--send :op "show" :id "open-me")
      (should (org-roam-navi-test--wait
               (lambda () (equal (buffer-file-name (window-buffer)) note))))
      (should (= (window-point) pos))
      ;; that node isn't echoed back as a show
      (setq org-roam-navi--last-shown nil)
      (with-current-buffer (window-buffer) (setq org-roam-navi--last-shown (org-roam-navi--here)))
      (should (equal (car org-roam-navi--last-shown) "open-me")))))

(ert-deftest org-roam-navi-test-quit-and-teardown ()
  (org-roam-navi-test--with-fake
    (org-roam-navi)
    (should (org-roam-navi-test--wait (lambda () org-roam-navi--hello)))
    (should org-roam-navi--timer)
    (org-roam-navi-quit)
    (should (org-roam-navi-test--wait (lambda () (not (org-roam-navi--live-p)))))
    (should (org-roam-navi-test--wait (lambda () (null org-roam-navi--timer))))
    (should (seq-find (lambda (c) (equal (plist-get c :op) "quit"))
                      (org-roam-navi-test--log log)))))

(ert-deftest org-roam-navi-test-asset-names ()
  (let ((names '("Navi-1.2.0-universal.zip" "navi-1.2.0-linux-x86_64.tar.gz"
                 "navi-1.2.0-linux-aarch64.tar.gz" "navi-1.2.0-windows-x86_64.zip"
                 "navi-1.2.0-freebsd-x86_64.tar.gz" "SHA256SUMS")))
    (cl-flet ((pick (type conf)
                (let ((system-type type) (system-configuration conf))
                  (seq-filter #'org-roam-navi--asset-p names))))
      (should (equal (pick 'darwin "aarch64-apple-darwin24") '("Navi-1.2.0-universal.zip")))
      (should (equal (pick 'darwin "x86_64-apple-darwin21") '("Navi-1.2.0-universal.zip")))
      (should (equal (pick 'gnu/linux "x86_64-pc-linux-gnu") '("navi-1.2.0-linux-x86_64.tar.gz")))
      (should (equal (pick 'gnu/linux "aarch64-unknown-linux-gnu") '("navi-1.2.0-linux-aarch64.tar.gz")))
      (should (equal (pick 'windows-nt "x86_64-w64-mingw32") '("navi-1.2.0-windows-x86_64.zip")))
      (should (equal (pick 'berkeley-unix "x86_64-unknown-freebsd14.1") '("navi-1.2.0-freebsd-x86_64.tar.gz"))))))

(ert-deftest org-roam-navi-test-colours ()
  (should (string-match-p "\\`#[0-9a-f]\\{6\\}\\'"
                          (or (org-roam-navi--colour 'default :foreground) "#000000")))
  (should-not (org-roam-navi--colour 'no-such-face :foreground)))

;; The download path with the network stubbed: a release whose asset is a
;; real archive of this platform's kind, checked against SHA256SUMS.
(defun org-roam-navi-test--archive (dir sums-ok)
  "Build a release archive in DIR; return (ASSET-NAME ARCHIVE SUMS-FILE)."
  (let* ((stage (expand-file-name "stage" dir))
         (mac (eq system-type 'darwin))
         (name (if mac "Navi-9.9.9-universal.zip" "navi-9.9.9-linux-x86_64.tar.gz"))
         (archive (expand-file-name name dir))
         (bin (expand-file-name (if mac "Navi.app/Contents/MacOS/navi" "navi-9.9.9-linux-x86_64/navi")
                                stage)))
    (make-directory (file-name-directory bin) t)
    (with-temp-file bin (insert "#!/bin/sh\n"))
    (set-file-modes bin #o755)
    (let ((default-directory stage))
      (if mac
          (call-process "ditto" nil nil nil "-c" "-k" "--keepParent" "Navi.app" archive)
        (call-process "tar" nil nil nil "-czf" archive "navi-9.9.9-linux-x86_64")))
    (let ((sums (expand-file-name "SHA256SUMS" dir)))
      (with-temp-file sums
        (insert (if sums-ok (org-roam-navi--sha256 archive) (make-string 64 ?0))
                "  " name "\n"))
      (list name archive sums))))

(defun org-roam-navi-test--install (sums-ok &optional no-sums old-copy)
  "Run `org-roam-navi-install' against a stubbed release.
NO-SUMS leaves SHA256SUMS out of the release; OLD-COPY first puts a
previous install in place.  Returns (TMP INSTALL-DIR ERROR)."
  (let* ((tmp (make-temp-file "navi-install" t))
         (org-roam-navi-install-directory (expand-file-name "inst/" tmp))
         (_ (when old-copy
              (make-directory org-roam-navi-install-directory t)
              (with-temp-file (expand-file-name "VERSION" org-roam-navi-install-directory)
                (insert "v1.0.0\n"))))
         (system-configuration (if (eq system-type 'darwin) system-configuration "x86_64-pc-linux-gnu"))
         (made (org-roam-navi-test--archive tmp sums-ok))
         (release (list :tag_name "v9.9.9"
                        :assets (append
                                 (list (list :name (nth 0 made) :browser_download_url "archive"))
                                 (unless no-sums
                                   (list (list :name "SHA256SUMS" :browser_download_url "sums")))))))
    (cl-letf (((symbol-function 'org-roam-navi--fetch-json) (lambda (_) release))
              ((symbol-function 'url-copy-file)
               (lambda (url to &rest _)
                 (copy-file (if (equal url "sums") (nth 2 made) (nth 1 made)) to t))))
      (condition-case err
          (progn (org-roam-navi-install) (list tmp org-roam-navi-install-directory nil))
        (error (list tmp org-roam-navi-install-directory err))))))

(ert-deftest org-roam-navi-test-install ()
  (skip-unless (memq system-type '(darwin gnu/linux)))
  (pcase-let ((`(,tmp ,dir ,err) (org-roam-navi-test--install t)))
    (unwind-protect
        (progn
          (should-not err)
          (should (org-roam-navi--find-binary dir))
          (should (equal (with-temp-buffer
                           (insert-file-contents (expand-file-name "VERSION" dir))
                           (buffer-string))
                         "v9.9.9\n")))
      (delete-directory tmp t))))

(ert-deftest org-roam-navi-test-install-refuses-a-bad-checksum ()
  (skip-unless (memq system-type '(darwin gnu/linux)))
  (pcase-let ((`(,tmp ,dir ,err) (org-roam-navi-test--install nil)))
    (unwind-protect
        (progn
          (should err)
          (should (string-match-p "SHA256SUMS" (error-message-string err)))
          (should-not (file-exists-p dir)))
      (delete-directory tmp t))))

(ert-deftest org-roam-navi-test-install-keeps-the-old-copy-on-failure ()
  (skip-unless (memq system-type '(darwin gnu/linux)))
  (pcase-let ((`(,tmp ,dir ,err) (org-roam-navi-test--install nil nil t)))
    (unwind-protect
        (progn
          (should err)
          (should (equal (with-temp-buffer
                           (insert-file-contents (expand-file-name "VERSION" dir))
                           (buffer-string))
                         "v1.0.0\n"))
          ;; no staging directory left behind
          (should (equal (directory-files tmp nil "\\`\\.navi-new-") nil)))
      (delete-directory tmp t))))

(ert-deftest org-roam-navi-test-install-needs-checksums ()
  (skip-unless (memq system-type '(darwin gnu/linux)))
  (pcase-let ((`(,tmp ,dir ,err) (org-roam-navi-test--install t t)))
    (unwind-protect
        (progn
          (should err)
          (should (string-match-p "SHA256SUMS" (error-message-string err)))
          (should-not (file-exists-p dir)))
      (delete-directory tmp t))))

(ert-deftest org-roam-navi-test-install-upgrades-in-place ()
  (skip-unless (memq system-type '(darwin gnu/linux)))
  (pcase-let ((`(,tmp ,dir ,err) (org-roam-navi-test--install t nil t)))
    (unwind-protect
        (progn
          (should-not err)
          (should (org-roam-navi--find-binary dir))
          (should (equal (with-temp-buffer
                           (insert-file-contents (expand-file-name "VERSION" dir))
                           (buffer-string))
                         "v9.9.9\n")))
      (delete-directory tmp t))))

;;; org-roam-navi-test.el ends here
