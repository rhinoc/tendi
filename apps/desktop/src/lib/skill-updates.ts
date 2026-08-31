import type { RuntimeData } from "./data.ts";
import { applySkillUpdateReports as applySkillUpdateReportController } from "../controllers/skill-controller.ts";

export type SkillUpdateReport = {
  id?: string;
  name: string;
  status: string;
};

export function applySkillUpdateReportsToData(data: RuntimeData, updates: SkillUpdateReport[]): RuntimeData {
  return applySkillUpdateReportController(data, updates);
}
