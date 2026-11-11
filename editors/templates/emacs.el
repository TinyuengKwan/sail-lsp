;;; sail-lsp.el --- Managed Sail integration -*- lexical-binding: t; -*-
;; Emacs 29+ includes Eglot.
(require 'eglot)
(unless (fboundp 'sail-mode)
  (define-derived-mode sail-mode prog-mode "Sail" "Edit Sail source."))
(unless (fboundp 'sail-project-mode)
  (define-derived-mode sail-project-mode prog-mode "Sail Project" "Edit Sail project manifests."))
(add-to-list 'auto-mode-alist '("\\.sail\\'" . sail-mode))
(add-to-list 'auto-mode-alist '("\\.sail_project\\'" . sail-project-mode))
(add-to-list 'auto-mode-alist '("/sail\\.proj\\'" . sail-project-mode))
(add-to-list 'eglot-server-programs '((sail-mode sail-project-mode) . (@SERVER_JSON@)))
(add-hook 'sail-mode-hook #'eglot-ensure)
(add-hook 'sail-project-mode-hook #'eglot-ensure)

;; Settings travel as the `sail-lsp' section of `workspace/configuration'.
;; Eglot reads them from `eglot-workspace-configuration', which is best set
;; per project in a `.dir-locals.el'; the keys are those of the VS Code
;; settings without the `sail-lsp.' prefix, as nested plists:
;;
;;   ((nil . ((eglot-workspace-configuration
;;             . (:sail-lsp (:diagnostics (:disabled ["unused-variable"])
;;                           :completion (:limit 100)
;;                           :workspace (:projectFile "model/riscv.sail_project")))))))
;;
;; A `.sail-lsp.toml' in the workspace folder is read by the server itself
;; and needs no Emacs configuration at all.

;; The server's own requests, which Eglot does not know about. Each sends
;; the request the VS Code command of the same name sends.
(defun sail-lsp--request (method &optional params)
  "Send METHOD with PARAMS to this buffer's Sail server and return the result."
  (jsonrpc-request (eglot--current-server-or-lose) method params))

(defun sail-lsp--uri-to-path (uri)
  (funcall (if (fboundp 'eglot-uri-to-path) #'eglot-uri-to-path #'eglot--uri-to-path) uri))

(defun sail-lsp--goto-location (location)
  "Open the file of LOCATION, an LSP Location plist, at its range start."
  (find-file (sail-lsp--uri-to-path (plist-get location :uri)))
  (goto-char (eglot--lsp-position-to-point
              (plist-get (plist-get location :range) :start))))

(defun sail-lsp-analyzer-status ()
  "Show the server's status report (VS Code: Sail: Show Server Status)."
  (interactive)
  (let ((status (sail-lsp--request :sail-lsp/analyzerStatus)))
    (with-current-buffer (get-buffer-create "*sail-lsp status*")
      (let ((inhibit-read-only t))
        (erase-buffer)
        (insert status))
      (special-mode)
      (display-buffer (current-buffer)))))

(defun sail-lsp-open-project-manifest ()
  "Open the manifest of the project this file belongs to."
  (interactive)
  (let ((location (sail-lsp--request
                   :sail-lsp/openProjectManifest
                   (list :textDocument (eglot--TextDocumentIdentifier)))))
    (if location
        (sail-lsp--goto-location location)
      (message "This file is not part of a loaded Sail project."))))

(defun sail-lsp-parent-module ()
  "Go to where the project lists or includes this file."
  (interactive)
  (let ((locations (append (sail-lsp--request
                            :sail-lsp/parentModule
                            (list :textDocument (eglot--TextDocumentIdentifier)))
                           nil)))
    (pcase (length locations)
      (0 (message "Nothing in the project lists or includes this file."))
      (1 (sail-lsp--goto-location (car locations)))
      (_ (let* ((names (mapcar (lambda (l) (sail-lsp--uri-to-path (plist-get l :uri)))
                               locations))
                (chosen (completing-read "Parent: " names nil t)))
           (sail-lsp--goto-location (nth (cl-position chosen names :test #'equal)
                                         locations)))))))

(defun sail-lsp-join-lines ()
  "Join the lines of the region, or the line at point with the next one."
  (interactive)
  (let* ((start (if (use-region-p) (region-beginning) (point)))
         (end (if (use-region-p) (region-end) (point)))
         (edits (sail-lsp--request
                 :experimental/joinLines
                 (list :textDocument (eglot--TextDocumentIdentifier)
                       :ranges (vector (list :start (eglot--pos-to-lsp-position start)
                                             :end (eglot--pos-to-lsp-position end)))))))
    (eglot--apply-text-edits edits)))
