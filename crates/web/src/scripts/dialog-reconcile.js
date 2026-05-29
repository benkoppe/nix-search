(() => {
  const dialog = document.getElementById("entry-modal");

  if (dialog) {
    if (!dialog.open) dialog.showModal();
  } else {
    document.querySelectorAll("dialog[open]").forEach((d) => d.close());
  }

  if (window.nixsearchSyncModalState) {
    window.nixsearchSyncModalState();
  } else {
    document.documentElement.classList.toggle(
      "modal-open",
      !!dialog && dialog.open
    );
  }

  if (!dialog && window.nixsearchRestoreResultFocus) {
    window.nixsearchRestoreResultFocus();
  }
})();
