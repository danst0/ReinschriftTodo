document.body.classList.add("js");

// copy-to-clipboard for command chips
document.querySelectorAll(".cmd[data-copy]").forEach((btn) => {
  btn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(btn.dataset.copy);
      if (!btn.querySelector(".copied")) {
        const ok = document.createElement("span");
        ok.className = "copied";
        ok.textContent = btn.closest("[lang=de], html[lang=de]") ? "kopiert ✓" : "copied ✓";
        btn.appendChild(ok);
        setTimeout(() => ok.remove(), 1600);
      }
    } catch (e) {
      /* clipboard unavailable — leave the text selectable */
    }
  });
});

// gentle reveal on scroll
const io = new IntersectionObserver(
  (entries) => {
    entries.forEach((en) => {
      if (en.isIntersecting) {
        en.target.classList.add("in");
        io.unobserve(en.target);
      }
    });
  },
  { rootMargin: "0px 0px -8% 0px" }
);
document.querySelectorAll(".reveal").forEach((el) => io.observe(el));
