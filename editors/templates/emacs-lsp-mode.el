;;; sail-lsp-mode.el --- Sail with lsp-mode -*- lexical-binding: t; -*-
;; The lsp-mode counterpart of emacs.el. `sail-lsp install --editor emacs'
;; writes the Eglot file; load this one instead if you use lsp-mode.
(require 'lsp-mode)
(unless (fboundp 'sail-mode)
  (define-derived-mode sail-mode prog-mode "Sail" "Edit Sail source."))
(unless (fboundp 'sail-project-mode)
  (define-derived-mode sail-project-mode prog-mode "Sail Project" "Edit Sail project manifests."))
(add-to-list 'auto-mode-alist '("\\.sail\\'" . sail-mode))
(add-to-list 'auto-mode-alist '("\\.sail_project\\'" . sail-project-mode))
(add-to-list 'auto-mode-alist '("/sail\\.proj\\'" . sail-project-mode))
(add-to-list 'lsp-language-id-configuration '(sail-mode . "sail"))
(add-to-list 'lsp-language-id-configuration '(sail-project-mode . "sail-project"))

;; Settings are the VS Code keys. lsp-mode sends the `sail-lsp' section as
;; `initializationOptions' and answers `workspace/configuration' with it;
;; declare the ones you set, each bound to a variable or a value.
(defcustom lsp-sail-lsp-diagnostics-disabled []
  "Native diagnostic codes to suppress, e.g. [\"unused-variable\"]."
  :type 'lsp-string-vector :group 'lsp-mode)
(defcustom lsp-sail-lsp-completion-limit 200
  "Maximum number of completion items."
  :type 'integer :group 'lsp-mode)
(lsp-register-custom-settings
 '(("sail-lsp.diagnostics.disabled" lsp-sail-lsp-diagnostics-disabled)
   ("sail-lsp.completion.limit" lsp-sail-lsp-completion-limit)))

(lsp-register-client
 (make-lsp-client
  :new-connection (lsp-stdio-connection '("sail-lsp"))
  :activation-fn (lsp-activate-on "sail" "sail-project")
  :server-id 'sail-lsp
  :initialization-options (lambda () (lsp-configuration-section "sail-lsp"))))
(add-hook 'sail-mode-hook #'lsp-deferred)
(add-hook 'sail-project-mode-hook #'lsp-deferred)

;; The server's own requests, as the VS Code commands send them.
(defun sail-lsp-analyzer-status ()
  "Show the server's status report."
  (interactive)
  (let ((status (lsp-request "sail-lsp/analyzerStatus" nil)))
    (with-current-buffer (get-buffer-create "*sail-lsp status*")
      (let ((inhibit-read-only t))
        (erase-buffer)
        (insert status))
      (special-mode)
      (display-buffer (current-buffer)))))

(defun sail-lsp-open-project-manifest ()
  "Open the manifest of the project this file belongs to."
  (interactive)
  (let ((location (lsp-request "sail-lsp/openProjectManifest"
                               (list :textDocument (lsp--text-document-identifier)))))
    (if location
        (lsp-show-xrefs (lsp--locations-to-xref-items (vector location)) nil nil)
      (message "This file is not part of a loaded Sail project."))))

(defun sail-lsp-parent-module ()
  "Go to where the project lists or includes this file."
  (interactive)
  (let ((locations (lsp-request "sail-lsp/parentModule"
                                (list :textDocument (lsp--text-document-identifier)))))
    (if (seq-empty-p locations)
        (message "Nothing in the project lists or includes this file.")
      (lsp-show-xrefs (lsp--locations-to-xref-items locations) nil nil))))

(defun sail-lsp-join-lines ()
  "Join the lines of the region, or the line at point with the next one."
  (interactive)
  (let* ((start (if (use-region-p) (region-beginning) (point)))
         (end (if (use-region-p) (region-end) (point)))
         (edits (lsp-request "experimental/joinLines"
                             (list :textDocument (lsp--text-document-identifier)
                                   :ranges (vector (list :start (lsp-point-to-position start)
                                                         :end (lsp-point-to-position end)))))))
    (lsp--apply-text-edits edits)))
