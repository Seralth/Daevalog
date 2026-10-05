// Settings: the support modal.
"use strict";

Object.assign(DpsApp.prototype, {
  isChineseLanguage(lang) {
    return String(lang || "").startsWith("zh");
  },

  updateSupportVisibility() {
    if (this.supportWidget) {
      this.supportWidget.style.display = "flex";
    }
    if (this.kofiWidget) {
      this.kofiWidget.style.display = "none";
    }
  },

  getSupportIconSvg(type) {
    const iconByType = {
      afdian:
        '<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="M13.5 2 5 13h5l-1.5 9L19 10h-5.5L13.5 2z" fill="currentColor"/></svg>',
      kofi:
        '<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="M4 7h12v7a5 5 0 0 1-5 5H9a5 5 0 0 1-5-5V7z" fill="currentColor"/><path d="M16 9h1.5a2.5 2.5 0 0 1 0 5H16V9z" fill="currentColor" opacity="0.75"/><path d="M6 5h8" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" fill="none"/></svg>',
      wechat:
        '<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="M9 4c-3.87 0-7 2.69-7 6 0 1.9 1.03 3.59 2.62 4.69L4 19l3.66-1.85A8.4 8.4 0 0 0 9 17c3.87 0 7-2.69 7-6s-3.13-7-7-7z" fill="currentColor"/><path d="M16.5 10.5c3.04 0 5.5 2.01 5.5 4.5 0 1.42-.79 2.69-2.02 3.52L20.5 22l-2.79-1.41c-.39.08-.79.12-1.21.12-3.04 0-5.5-2.01-5.5-4.5s2.46-4.5 5.5-4.5z" fill="currentColor" opacity="0.78"/></svg>',
      paypal:
        '<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false"><path d="M7.2 3.2h7.2c3.2 0 5.3 1.9 4.9 4.7-.42 2.85-2.8 4.58-6.1 4.58h-2.4l-.75 4.35H5.9L7.2 3.2z" fill="currentColor"/><path d="M9.3 5.6h4.05c1.3 0 2.05.74 1.83 1.82-.22 1.13-1.2 1.84-2.49 1.84H8.55L9.3 5.6z" fill="currentColor" opacity="0.55"/><path d="M10.55 9.05h2.52c1.95 0 3.2 1.14 2.93 2.86-.3 1.92-1.95 3.12-4.19 3.12h-2.38l.56-3.18h2.2c.75 0 1.23-.4 1.33-1 .1-.56-.3-.93-1.02-.93h-2.15l.2-.87z" fill="#0b2f63" opacity="0.45"/></svg>',
    };
    return iconByType[type] || "";
  },

  updateSupportPrimaryAction(lang) {
    if (!this.supportPrimaryButton) return;
    const isChinese = this.isChineseLanguage(lang);
    const nextSupport = isChinese ? "afdian" : "kofi";
    const nextUrl = isChinese
      ? "https://afdian.com/a/hiddencube"
      : "https://ko-fi.com/hiddencube";
    const nextLabel = isChinese ? "爱发电" : "Ko-fi";
    const nextIcon = this.getSupportIconSvg(isChinese ? "afdian" : "kofi");
    this.supportPrimaryButton.dataset.support = nextSupport;
    this.supportPrimaryButton.dataset.url = nextUrl;
    const label = this.supportPrimaryButton.querySelector(".supportLabel");
    const icon = this.supportPrimaryButton.querySelector(".supportIcon");
    if (label) label.textContent = nextLabel;
    if (icon) icon.innerHTML = nextIcon;
    this.supportPrimaryButton.setAttribute("aria-label", nextLabel);
    const i18nLabel = isChinese ? "support.aria.afdian" : "support.aria.kofi";
    this.supportPrimaryButton.dataset.i18nAriaLabel = i18nLabel;
  },

  openSupportModal() {
    if (!this.supportModal) return;
    this.supportModal.classList.add("isOpen");
    this.supportModal.setAttribute("aria-hidden", "false");
    if (this.supportCopyStatus) {
      this.supportCopyStatus.textContent = "";
    }
  },

  closeSupportModal() {
    if (!this.supportModal) return;
    this.supportModal.classList.remove("isOpen");
    this.supportModal.setAttribute("aria-hidden", "true");
  },

  handleSupportAction(button) {
    if (!button) return;
    const supportType = button.dataset.support;
    const url = button.dataset.url;
    const copyValue = button.dataset.copy;
    const qrType = button.dataset.qr || supportType;

    if (qrType && this.supportQrImages?.[qrType]) {
      this.updateSupportQrImage(qrType);
    }

    if (url) {
      const externalOnly = supportType === "paypal" || supportType === "afdian" || supportType === "kofi";
      this.openExternalLink(url, { externalOnly });
    }

    if (copyValue) {
      const messageKey = `support.copy.${supportType}`;
      const fallback = `Copied ${supportType?.toUpperCase?.() || "address"}`;
      this.copySupportValue(copyValue, this.i18n?.t?.(messageKey, fallback) || fallback);
    }
  },

  openExternalLink(url, { externalOnly = false } = {}) {
    if (!url) return;
    window.javaBridge?.openBrowser?.(url);
    if (externalOnly) return;
    try {
      window.open(url, "_blank", "noopener");
    } catch {
      // ignore
    }
  },

  copySupportValue(value, message) {
    if (!value) return;
    const showStatus = (text) => {
      if (!this.supportCopyStatus) return;
      this.supportCopyStatus.textContent = text;
    };
    const attemptLegacyCopy = () => {
      try {
        const textarea = document.createElement("textarea");
        textarea.value = value;
        textarea.style.position = "fixed";
        textarea.style.opacity = "0";
        document.body.appendChild(textarea);
        textarea.select();
        const success = document.execCommand("copy");
        document.body.removeChild(textarea);
        if (success) showStatus(message);
      } catch {
        // ignore
      }
    };
    if (navigator.clipboard?.writeText) {
      navigator.clipboard
        .writeText(value)
        .then(() => showStatus(message))
        .catch(() => attemptLegacyCopy());
      return;
    }
    attemptLegacyCopy();
  },

  updateSupportQrImage(type) {
    if (!this.supportQrImage) return;
    const src = this.supportQrImages?.[type];
    if (!src) return;
    this.supportQrImage.src = src;
    this.updateSupportTitle(type);
    this.supportActionButtons?.forEach((button) => {
      const match = button.dataset.support === type || button.dataset.qr === type;
      button.classList.toggle("isActive", match);
    });
  },

  updateSupportTitle(type) {
    if (!this.supportModalTitle) return;
    const currentLanguage = this.i18n?.getLanguage?.();
    const isChinese = this.isChineseLanguage(currentLanguage);
    const isWeChat = type === "wechat";
    const titleKey = isWeChat ? "support.titleWechat" : "support.title";
    const fallback = isWeChat
      ? "Tip taengu, the A2Tools developer, on WeChat"
      : isChinese
        ? "Tip taengu, the A2Tools developer, on Afdian"
        : "Tip taengu, the A2Tools developer, on Ko-fi";
    this.supportModalTitle.textContent = this.i18n?.t?.(titleKey, fallback) || fallback;
  },
});
