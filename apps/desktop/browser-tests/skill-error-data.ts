let attempts = 0
export function getProjectClient(directory: string | null) {
 if (directory !== "/skill-error-fixture") throw new Error("Unexpected skill fixture request")
 return { app: { skills: async () => {
  attempts += 1
  const mode = new URLSearchParams(location.search).get("mode")
  if (mode === "failure" && attempts <= 2) throw new Error("Controlled skills transport failure")
  return { data: mode === "empty" ? [] : [{ name: "alpha", description: "Recovered skill" }] }
 } } }
}
