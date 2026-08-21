const retractLogo = new URL("../../src-tauri/icons/128x128.png", import.meta.url).href;

export function BrandLogo() {
  return (
    <span className="brand-mark">
      <img
        className="brand-logo"
        src={retractLogo}
        alt="Retract app logo"
        width="31"
        height="31"
        draggable={false}
      />
    </span>
  );
}
