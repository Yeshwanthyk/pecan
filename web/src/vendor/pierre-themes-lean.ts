import { normalizeTheme } from "shiki/core";

type ThemeDescriptor = {
  name: string;
  load: () => Promise<unknown>;
};

export function createTheme({
  name,
  load,
}: {
  name: string;
  load: () => Promise<unknown>;
}) {
  return {
    name,
    load: async () => {
      const loaded = await load();
      const value =
        typeof loaded === "object" && loaded !== null && "default" in loaded
          ? loaded.default
          : loaded;
      return normalizeTheme(value as Parameters<typeof normalizeTheme>[0]);
    },
  };
}

function collection(descriptors: ThemeDescriptor[]) {
  const byName = new Map(descriptors.map((descriptor) => [descriptor.name, descriptor]));
  return {
    getTheme: (name: string) => byName.get(name),
    getThemes: () => descriptors,
  };
}

export const pierreThemes = collection([
  createTheme({ name: "pierre-dark", load: () => import("@pierre/theme/pierre-dark") }),
  createTheme({ name: "pierre-light", load: () => import("@pierre/theme/pierre-light") }),
]);

export const shikiThemes = collection([]);
