// Injected into the Gemini headless page to paste an inline image, type a
// prompt, and submit. The caller replaces __IMAGE_BASE64__, __IMAGE_MIME__,
// __IMAGE_FILENAME__, and __PROMPT__.
(async function () {
  function isVisible(el) {
    if (!el) return false;
    const rect = el.getBoundingClientRect && el.getBoundingClientRect();
    if (!rect) return false;
    return rect.width > 0 && rect.height > 0;
  }

  function rejectClipboard(el) {
    if (!el) return true;
    const cls = (el.className || '').toString();
    if (cls.includes('ql-clipboard')) return true;
    const ariaLabel = (el.getAttribute('aria-label') || '').toLowerCase();
    if (ariaLabel && ariaLabel.includes('clipboard')) return true;
    return false;
  }

  function findInput() {
    const selectors = [
      'textarea.gds-body-l[placeholder="Ask Gemini"]',
      'textarea[placeholder="Ask Gemini"]',
      '.initial-input-area textarea',
      '.ql-editor.textarea[contenteditable="true"]',
      '.ql-editor[contenteditable="true"][aria-label*="prompt"]',
      '.ql-editor[contenteditable="true"][aria-label*="Prompt"]',
      '.ql-editor[contenteditable="true"]',
      '[contenteditable="true"][aria-label*="prompt"]',
      '[contenteditable="true"][aria-label*="Prompt"]',
      '[data-test-id="textarea-inner"] rich-textarea [contenteditable="true"]',
      '[data-test-id="textarea-wrapper"] rich-textarea [contenteditable="true"]',
      'rich-textarea [contenteditable="true"]',
      'rich-textarea',
      '[data-test-id="input-text"]',
      '[data-test-id="chat-input-textbox"]',
      'textarea',
      'input[type="text"]',
      '[contenteditable="true"]',
      '[role="textbox"]',
    ];
    for (const s of selectors) {
      const nodes = Array.from(document.querySelectorAll(s));
      for (const el of nodes) {
        if (rejectClipboard(el)) continue;
        if (!isVisible(el)) continue;
        return el;
      }
    }
    return null;
  }

  function findSendButton() {
    const selectors = [
      '.initial-input-area .send-icon',
      '.initial-input-area-container .send-icon',
      '.initial-input-area mat-icon',
      'button[aria-label*="Send"]',
      'button[aria-label*="send"]',
      'button[data-test-id="send-button"]',
      'button.send-button',
      'button[aria-label="Send message"]',
      '[data-test-id="send-button"]',
      '[data-test-id="submit-button"]',
    ];
    for (const s of selectors) {
      const el = document.querySelector(s);
      if (el && isVisible(el)) return el;
    }
    const buttons = Array.from(document.querySelectorAll('button'));
    return (
      buttons.find((b) => {
        if (!isVisible(b) || b.disabled) return false;
        const text = (b.textContent || b.getAttribute('aria-label') || '').toLowerCase();
        return text.includes('send') || text.includes('submit');
      }) || null
    );
  }

  const logs = [];
  function log(level, message) {
    const line = `[${level}] ${message}`;
    logs.push(line);
    if (typeof console !== 'undefined' && console.log) console.log(line);
  }

  function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  async function waitForInput(maxMs) {
    const deadline = Date.now() + maxMs;
    while (Date.now() < deadline) {
      const el = findInput();
      if (el) return el;
      await sleep(200);
    }
    return null;
  }

  log('info', 'waiting for Gemini input element...');
  const input = await waitForInput(15000);
  if (!input) {
    log('error', 'could not find a visible Gemini input element');
    window.__geminiSimLogs = logs;
    return { ok: false };
  }
  log('info', `input found: ${input.tagName} class="${input.className}"`);

  input.focus();
  input.click();
  await sleep(300);

  const prompt = '__PROMPT__';
  const imageBase64 = '__IMAGE_BASE64__';
  const imageMime = '__IMAGE_MIME__';
  const imageFilename = '__IMAGE_FILENAME__';
  let imageAttached = false;

  async function clickUploadMenu() {
    if (!imageBase64) return false;
    try {
      // Open the "Upload & tools" menu beside the input.
      const uploadBtn =
        document.querySelector('button[aria-label*="Upload" i]') ||
        document.querySelector('button[aria-label*="upload" i]') ||
        Array.from(document.querySelectorAll('button')).find((b) => {
          const label = (b.getAttribute('aria-label') || '').toLowerCase();
          return label.includes('upload') || label.includes('attach');
        });
      if (!uploadBtn) {
        log('info', 'no Upload & tools button found');
        return false;
      }
      uploadBtn.click();
      log('info', 'clicked Upload & tools button');
      await sleep(500);

      // Click the "Upload files" menu item.
      const menuItems = Array.from(document.querySelectorAll('button, [role="menuitem"], [role="menuitemradio"], [role="option"]'));
      const uploadFilesItem = menuItems.find((el) => {
        const text = (el.textContent || el.getAttribute('aria-label') || '').toLowerCase();
        return text.includes('upload files') || text.includes('upload file');
      });
      if (uploadFilesItem) {
        uploadFilesItem.click();
        log('info', 'clicked Upload files menu item');
      } else {
        // Some variants show a direct file input after clicking the upload icon.
        log('info', 'no Upload files menu item, relying on file input');
      }
      await sleep(500);
      return true;
    } catch (e) {
      log('warn', `upload menu failed: ${e && e.message ? e.message : e}`);
      return false;
    }
  }

  async function attachViaClipboard() {
    if (!imageBase64) return false;
    try {
      const binary = atob(imageBase64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);

      if (typeof navigator !== 'undefined' && navigator.clipboard && navigator.clipboard.write) {
        const blob = new Blob([bytes], { type: imageMime });
        const item = { [blob.type]: blob };
        try {
          // Some Chromium builds expose a synthetic ClipboardItem constructor;
          // try it first.
          const ClipboardItem = window.ClipboardItem || (window.DataTransferItem && window.DataTransferItem.prototype.constructor);
          if (ClipboardItem && ClipboardItem.length === 1) {
            await navigator.clipboard.write([new ClipboardItem(item)]);
          } else {
            await navigator.clipboard.write([item]);
          }
          input.focus();
          input.dispatchEvent(new KeyboardEvent('keydown', { key: 'v', code: 'KeyV', ctrlKey: true, bubbles: true, cancelable: true }));
          input.dispatchEvent(new KeyboardEvent('keyup', { key: 'v', code: 'KeyV', ctrlKey: true, bubbles: true, cancelable: true }));
          log('info', `dispatched async clipboard paste (${bytes.length} bytes)`);
          return true;
        } catch (e) {
          log('info', `navigator.clipboard paste not available: ${e && e.message ? e.message : e}`);
        }
      }
      return false;
    } catch (e) {
      log('warn', `clipboard attach failed: ${e && e.message ? e.message : e}`);
      return false;
    }
  }

  async function attachViaFileInput() {
    if (!imageBase64) return false;
    try {
      const binary = atob(imageBase64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      const blob = new Blob([bytes], { type: imageMime });
      const file = new File([blob], imageFilename, { type: imageMime });

      // Gemini hides a file input inside the chat input area. Prefer one that
      // is near the input element and not the generic page uploaders.
      const fileInput =
        input.closest('chat-input')?.querySelector('input[type="file"]') ||
        input.closest('.input-area')?.querySelector('input[type="file"]') ||
        input.closest('.initial-input-area')?.querySelector('input[type="file"]') ||
        input.closest('[data-test-id="textarea-wrapper"]')?.querySelector('input[type="file"]') ||
        input.closest('[data-test-id="input-text"]')?.querySelector('input[type="file"]') ||
        document.querySelector('input[type="file"]');
      if (fileInput) {
        const dt = new DataTransfer();
        dt.items.add(file);
        fileInput.files = dt.files;
        fileInput.dispatchEvent(new Event('change', { bubbles: true }));
        fileInput.dispatchEvent(new Event('input', { bubbles: true }));
        log('info', `set file input (${bytes.length} bytes) on ${fileInput.className || fileInput.id || fileInput.name || 'file'}`);
        return true;
      }
      return false;
    } catch (e) {
      log('warn', `file input attach failed: ${e && e.message ? e.message : e}`);
      return false;
    }
  }

  async function attachViaDragDrop() {
    if (!imageBase64) return false;
    try {
      const binary = atob(imageBase64);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
      const blob = new Blob([bytes], { type: imageMime });
      const file = new File([blob], imageFilename, { type: imageMime });

      const dt = new DataTransfer();
      dt.items.add(file);

      // The global drop zone is usually on the chat container. Target it if
      // present, otherwise fall back to the input element.
      const dropTarget =
        document.querySelector('.xap-uploader-dropzone') ||
        document.querySelector('[file-drop-zone]') ||
        input;
      const rect = dropTarget.getBoundingClientRect();
      const x = rect.left + rect.width / 2;
      const y = rect.top + rect.height / 2;

      const dragEnter = new DragEvent('dragenter', {
        bubbles: true,
        cancelable: true,
        dataTransfer: dt,
        clientX: x,
        clientY: y,
      });
      const dragOver = new DragEvent('dragover', {
        bubbles: true,
        cancelable: true,
        dataTransfer: dt,
        clientX: x,
        clientY: y,
      });
      const dropEvent = new DragEvent('drop', {
        bubbles: true,
        cancelable: true,
        dataTransfer: dt,
        clientX: x,
        clientY: y,
      });
      dropTarget.dispatchEvent(dragEnter);
      dropTarget.dispatchEvent(dragOver);
      dropTarget.dispatchEvent(dropEvent);
      input.dispatchEvent(dragEnter);
      input.dispatchEvent(dragOver);
      input.dispatchEvent(dropEvent);
      document.body.dispatchEvent(dragEnter);
      document.body.dispatchEvent(dragOver);
      document.body.dispatchEvent(dropEvent);
      log('info', `dispatched drag+drop (${bytes.length} bytes) on ${dropTarget.className || dropTarget.tagName}`);
      return true;
    } catch (e) {
      log('warn', `dragdrop attach failed: ${e && e.message ? e.message : e}`);
      return false;
    }
  }

  if (imageBase64) {
    // Try the real UI flow first: open Upload & tools -> Upload files, then
    // fill the hidden file input. Playwright proved this produces a valid
    // slot0[3] attachment array.
    const menuOpened = await clickUploadMenu();
    imageAttached = await attachViaFileInput();
    if (!imageAttached && menuOpened) {
      // The file input may appear asynchronously; wait briefly.
      for (let i = 0; i < 10 && !imageAttached; i++) {
        await sleep(300);
        imageAttached = await attachViaFileInput();
      }
    }
    if (!imageAttached) {
      imageAttached = await attachViaClipboard() || await attachViaDragDrop();
    }
    if (imageAttached) {
      // Give Gemini time to render the thumbnail and (if needed) upload the
      // file. The StreamGenerate request is only sent after the attachment is
      // ready.
      await sleep(3500);
    } else {
      log('warn', 'all image attach methods failed');
      await sleep(200);
    }
  } else {
    await sleep(200);
  }

  function setText(el, text) {
    if (!text) return true;
    if (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT') {
      el.value = text;
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
      return true;
    }
    el.tabIndex = 0;
    el.focus();
    if (imageAttached) {
      document.execCommand('selectAll', false, null);
      document.execCommand('insertText', false, text);
    } else {
      el.textContent = text;
      el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
    }
    el.dispatchEvent(new InputEvent('input', { bubbles: true, inputType: 'insertText', data: text }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
    return el.innerText.trim().length >= text.length / 2;
  }

  // For image turns, set text *after* the attachment so the inline image is
  // already present and the prompt text simply accompanies it.
  setText(input, prompt);
  log('info', `text set to: "${(input.innerText || input.value || '').slice(0, 80)}"`);

  // After setting text, wait for any upload/attestation to settle before
  // clicking send. This is especially important for image turns.
  await sleep(imageAttached ? 1500 : 200);
  const btn = findSendButton();
  let clickedSend = false;
  if (btn) {
    log('info', `clicking send button: ${btn.tagName}`);
    const gemBtn = btn.closest('gem-icon-button');
    (gemBtn || btn).click();
    clickedSend = true;
  } else {
    log('info', 'no send button found; relying on Enter fallback');
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', keyCode: 13, bubbles: true, cancelable: true }));
    input.dispatchEvent(new KeyboardEvent('keypress', { key: 'Enter', code: 'Enter', keyCode: 13, bubbles: true, cancelable: true }));
    input.dispatchEvent(new KeyboardEvent('keyup', { key: 'Enter', code: 'Enter', keyCode: 13, bubbles: true, cancelable: true }));
  }

  const rect = input.getBoundingClientRect();
  const result = {
    ok: true,
    tag: input.tagName,
    className: input.className,
    clickedSend: clickedSend,
    pasteReady: !!imageBase64,
    imageAttached: imageAttached,
    text: (input.innerText || input.value || '').slice(0, 200),
    rect: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
  };

  window.__geminiSimLogs = logs;
  return result;
})();
