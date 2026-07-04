---
layout: page
---

<script setup>
if (typeof window !== 'undefined') {
  const lang = navigator.language.startsWith('zh') ? '/zh/' : '/en/';
  window.location.href = lang;
}
</script>

<div style="display: flex; align-items: center; justify-content: center; min-height: 50vh;">
  <p style="color: var(--vp-c-text-2);">Redirecting…</p>
</div>
