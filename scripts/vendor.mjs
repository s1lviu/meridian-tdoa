import { cpSync, mkdirSync } from "node:fs";
mkdirSync("web/vendor", { recursive: true });
cpSync("node_modules/leaflet/dist", "web/vendor/leaflet", { recursive: true });
cpSync("node_modules/lucide/dist/umd/lucide.js", "web/vendor/lucide.js");

cpSync("node_modules/leaflet/LICENSE", "web/vendor/leaflet/LICENSE");
cpSync("node_modules/lucide/LICENSE", "web/vendor/lucide.LICENSE");
