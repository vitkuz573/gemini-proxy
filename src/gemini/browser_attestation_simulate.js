(async function () {
  const prompt = "__PROMPT__";

  function findInput() {
    const selectors = [
      'rich-textarea',
      '[data-test-id="input-text"]',
      '[data-test-id="chat-input-textbox"]',
      'textarea',
      'input[type="text"]',
      '[contenteditable="true"]',
      '[role="textbox"]',
    ];
    for (const s of selectors) {
      const el = document.querySelector(s);
      if (el) return el;
    }
    return null;
  }

  function findSendButton() {
    const selectors = [
      'button[aria-label*="Send"]',
      'button[aria-label*="send"]',
      'button[data-test-id="send-button"]',
      'button.send-button',
      'button[aria-label="Send message"]',
    ];
    for (const s of selectors) {
      const el = document.querySelector(s);
      if (el) return el;
    }
    const buttons = Array.from(document.querySelectorAll('button'));
    return (
      buttons.find((b) => {
        const text = (b.textContent || b.ariaLabel || '').toLowerCase();
        return text.includes('send') || text.includes('submit');
      }) || null
    );
  }

  function setValue(el, text) {
    if (el.tagName === 'TEXTAREA' || el.tagName === 'INPUT') {
      el.value = text;
    } else if (el.contentEditable === 'true' || el.getAttribute('role') === 'textbox') {
      el.textContent = text;
    } else if (el.shadowRoot) {
      const inner = el.shadowRoot.querySelector('textarea, input, [contenteditable="true"]');
      if (inner) setValue(inner, text);
    }
  }

  function dispatch(el, type, init) {
    const event = new Event(type, { bubbles: true, cancelable: true, ...init });
    el.dispatchEvent(event);
  }

  function dispatchKeyboard(el, type, key) {
    const event = new KeyboardEvent(type, {
      key,
      code: key === 'Enter' ? 'Enter' : key,
      keyCode: key === 'Enter' ? 13 : 0,
      which: key === 'Enter' ? 13 : 0,
      bubbles: true,
      cancelable: true,
    });
    el.dispatchEvent(event);
  }

  const input = findInput();
  if (!input) return false;

  // Wait until the input is focusable/visible.
  const isVisible = (el) => {
    const rect = el.getBoundingClientRect && el.getBoundingClientRect();
    return rect && rect.width > 0 && rect.height > 0;
  };
  let attempts = 0;
  while ((!input.offsetParent || !isVisible(input)) && attempts < 50) {
    await new Promise((r) => setTimeout(r, 100));
    attempts++;
  }
  if (!input.offsetParent || !isVisible(input)) return false;

  input.focus();
  input.click();
  setValue(input, prompt);
  dispatch(input, 'input', { inputType: 'insertText' });
  dispatch(input, 'change');
  dispatchKeyboard(input, 'keydown', 'Enter');
  dispatchKeyboard(input, 'keyup', 'Enter');

  // Some versions of the UI require an explicit send-button click.
  const btn = findSendButton();
  if (btn && !btn.disabled) {
    btn.focus();
    btn.click();
    dispatch(btn, 'mousedown');
    dispatch(btn, 'mouseup');
  }

  return true;
})();
