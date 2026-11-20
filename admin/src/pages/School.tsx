/**
 * School (§6.3, P9) — the school portal + CBT in the admin: courses
 * and lessons, enrolments, question banks, exams, the manual grading
 * queue, results, and certificates. The same page is the exam-taking
 * UI for a logged-in student: "My exams" lists the courses they are
 * enrolled in, starts an attempt, and shows the result.
 *
 * Honest limits, shown on the page: no live proctoring, no webcam
 * monitoring, no lockdown browser. The controls that ARE provided
 * are server-side timing, shuffled question order, attempt limits,
 * exam windows, and enrolment-only access.
 */
import { useCallback, useEffect, useState } from "react";
import { api } from "../lib/api";
import { formatDateTime } from "../lib/format";
import type {
  SchoolAttempt,
  SchoolCertificate,
  SchoolCourse,
  SchoolCourseList,
  SchoolEnrolment,
  SchoolExam,
  SchoolGradingItem,
  SchoolLesson,
  SchoolQuestion,
  SchoolQuestionBank,
  SchoolResult,
  User,
} from "../lib/types";
import {
  Alert,
  Badge,
  Button,
  Card,
  EmptyState,
  Input,
  Select,
  Skeleton,
  Table,
  Tabs,
} from "../components/elements";

type TabId = "courses" | "my-exams" | "grading" | "certificates";

export function SchoolPage({
  timezone,
  user,
}: {
  timezone: string;
  user: User;
}): React.ReactElement {
  const [tab, setTab] = useState<TabId>("courses");
  const [error, setError] = useState<string | null>(null);
  return (
    <div>
      {error !== null ? (
        <Alert tone="err" title="School" onDismiss={() => setError(null)}>
          {error}
        </Alert>
      ) : null}
      <p className="row-sub">
        No live proctoring, webcam monitoring, or lockdown browser. Exam integrity comes from
        server-side timing, shuffled question order, attempt limits, exam windows, and
        enrolment-only access.
      </p>
      <Tabs
        ariaLabel="School sections"
        value={tab}
        onChange={(id) => setTab(id as TabId)}
        tabs={[
          { id: "courses", label: "Courses" },
          { id: "my-exams", label: "My exams" },
          { id: "grading", label: "Grading queue" },
          { id: "certificates", label: "Certificates" },
        ]}
      />
      {tab === "courses" ? (
        <CoursesTab timezone={timezone} user={user} onError={setError} />
      ) : null}
      {tab === "my-exams" ? <MyExamsTab onError={setError} /> : null}
      {tab === "grading" ? <GradingTab timezone={timezone} onError={setError} /> : null}
      {tab === "certificates" ? <CertificatesTab timezone={timezone} onError={setError} /> : null}
    </div>
  );
}

function CoursesTab({
  timezone,
  user,
  onError,
}: {
  timezone: string;
  user: User;
  onError: (msg: string) => void;
}): React.ReactElement {
  const [list, setList] = useState<SchoolCourseList | null>(null);
  const [page, setPage] = useState(1);
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [creating, setCreating] = useState(false);
  const [selected, setSelected] = useState<SchoolCourse | null>(null);

  const load = useCallback(async () => {
    try {
      setList(await api.get<SchoolCourseList>(`/api/school/courses?page=${page}&per_page=20`));
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load courses.");
    }
  }, [page, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  const create = async (): Promise<void> => {
    if (title.trim() === "") return;
    setCreating(true);
    try {
      const course = await api.post<SchoolCourse>("/api/school/courses", { title, description });
      setTitle("");
      setDescription("");
      await load();
      setSelected(course);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to create course.");
    } finally {
      setCreating(false);
    }
  };

  const publish = async (c: SchoolCourse): Promise<void> => {
    try {
      await api.put<SchoolCourse>(`/api/school/courses/${c.id}`, {
        status: c.status === "published" ? "draft" : "published",
      });
      await load();
      setSelected(null);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to change course status.");
    }
  };

  const enrol = async (c: SchoolCourse): Promise<void> => {
    try {
      await api.post(`/api/school/courses/${c.id}/enrol`, {});
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to enrol.");
    }
  };

  return (
    <div>
      <Card title="Courses" sub={list !== null ? `${list.total} courses` : undefined}>
        <div className="toolbar">
          <Input label="New course title" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="e.g. Rust 101" />
          <Input label="Description" value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What the course covers…" />
          <Button icon="plus" disabled={creating || title.trim() === ""} onClick={() => void create()}>
            Create course
          </Button>
        </div>
        {list === null ? (
          <Skeleton height={180} label="Loading courses" />
        ) : list.items.length === 0 ? (
          <EmptyState icon="book" title="No courses yet" description="Create the first course above. A course starts as a draft — publish it before students can enrol." />
        ) : (
          <Table
            caption="Courses"
            columns={[
              { key: "course", label: "Course" },
              { key: "instructor", label: "Instructor" },
              { key: "lessons", label: "Lessons" },
              { key: "students", label: "Students" },
              { key: "status", label: "Status" },
              { key: "actions", label: "Actions" },
            ]}
            rows={list.items.map((c) => ({
              course: (
                <span>
                  <span className="row-title">{c.title}</span>
                  <span className="row-sub">{c.description}</span>
                </span>
              ),
              instructor: c.instructorName || `@${c.instructorLogin}`,
              lessons: String(c.lessonCount),
              students: String(c.enrolmentCount),
              status: <Badge tone={c.status === "published" ? "teal" : "amber"}>{c.status}</Badge>,
              actions: (
                <span>
                  <Button size="small" onClick={() => setSelected(c)}>Manage</Button>{" "}
                  {c.instructorId === user.id || user.role === "administrator" ? (
                    <Button size="small" onClick={() => void publish(c)}>
                      {c.status === "published" ? "Unpublish" : "Publish"}
                    </Button>
                  ) : null}{" "}
                  {c.status === "published" ? (
                    <Button size="small" onClick={() => void enrol(c)}>Enrol</Button>
                  ) : null}
                </span>
              ),
            }))}
          />
        )}
        {list !== null && list.total > 20 ? (
          <div className="pagination">
            <Button size="small" icon="chevronLeft" disabled={page <= 1} onClick={() => setPage(page - 1)}>
              Previous
            </Button>
            <span>Page {page}</span>
            <Button size="small" icon="chevronRight" disabled={list.items.length < 20} onClick={() => setPage(page + 1)}>
              Next
            </Button>
          </div>
        ) : null}
      </Card>
      {selected !== null ? (
        <CourseDetail course={selected} timezone={timezone} onError={onError} onClose={() => setSelected(null)} />
      ) : null}
    </div>
  );
}

function CourseDetail({
  course,
  timezone,
  onError,
  onClose,
}: {
  course: SchoolCourse;
  timezone: string;
  onError: (msg: string) => void;
  onClose: () => void;
}): React.ReactElement {
  const [lessons, setLessons] = useState<SchoolLesson[]>([]);
  const [enrolments, setEnrolments] = useState<SchoolEnrolment[]>([]);
  const [banks, setBanks] = useState<SchoolQuestionBank[]>([]);
  const [exams, setExams] = useState<SchoolExam[]>([]);
  const [lessonTitle, setLessonTitle] = useState("");
  const [lessonContent, setLessonContent] = useState("");
  const [bankTitle, setBankTitle] = useState("");
  const [examTitle, setExamTitle] = useState("");
  const [examBankId, setExamBankId] = useState("");
  const [selectedBank, setSelectedBank] = useState<SchoolQuestionBank | null>(null);

  const load = useCallback(async () => {
    try {
      const [l, e, b, x] = await Promise.all([
        api.get<{ items: SchoolLesson[] }>(`/api/school/courses/${course.id}/lessons`),
        api.get<{ items: SchoolEnrolment[] }>(`/api/school/courses/${course.id}/enrolments`),
        api.get<{ items: SchoolQuestionBank[] }>(`/api/school/courses/${course.id}/banks`),
        api.get<{ items: SchoolExam[] }>(`/api/school/courses/${course.id}/exams`),
      ]);
      setLessons(l.items);
      setEnrolments(e.items);
      setBanks(b.items);
      setExams(x.items);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load course detail.");
    }
  }, [course.id, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  const addLesson = async (): Promise<void> => {
    if (lessonTitle.trim() === "") return;
    try {
      await api.post(`/api/school/courses/${course.id}/lessons`, {
        title: lessonTitle,
        content: lessonContent,
      });
      setLessonTitle("");
      setLessonContent("");
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to add lesson.");
    }
  };

  const completeLesson = async (lessonId: number): Promise<void> => {
    try {
      await api.post(`/api/school/lessons/${lessonId}/complete`, {});
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to complete lesson.");
    }
  };

  const addBank = async (): Promise<void> => {
    if (bankTitle.trim() === "") return;
    try {
      await api.post(`/api/school/courses/${course.id}/banks`, { title: bankTitle });
      setBankTitle("");
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to add question bank.");
    }
  };

  const addExam = async (): Promise<void> => {
    if (examTitle.trim() === "" || examBankId === "") return;
    try {
      await api.post(`/api/school/courses/${course.id}/exams`, {
        title: examTitle,
        bankId: Number(examBankId),
        durationMinutes: 30,
        maxAttempts: 1,
        passMarkPercent: 50,
      });
      setExamTitle("");
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to add exam.");
    }
  };

  const publishExam = async (exam: SchoolExam): Promise<void> => {
    try {
      await api.put<SchoolExam>(`/api/school/exams/${exam.id}`, {
        status: exam.status === "published" ? "draft" : "published",
      });
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to change exam status.");
    }
  };

  return (
    <Card title={course.title} sub={`${course.slug} · ${course.status}`}>
      <div className="toolbar">
        <Button size="small" onClick={onClose}>Close</Button>
        <a className="row-sub" href={`/school/courses/${course.slug}`} target="_blank" rel="noreferrer">
          View public course page
        </a>
      </div>
      <h3>Lessons ({lessons.length})</h3>
      <div className="toolbar">
        <Input label="Lesson title" value={lessonTitle} onChange={(e) => setLessonTitle(e.target.value)} placeholder="Lesson title" />
        <Input label="Content" value={lessonContent} onChange={(e) => setLessonContent(e.target.value)} placeholder="Lesson content…" />
        <Button icon="plus" disabled={lessonTitle.trim() === ""} onClick={() => void addLesson()}>
          Add lesson
        </Button>
      </div>
      {lessons.length === 0 ? (
        <EmptyState icon="book" title="No lessons yet" description="Add the first lesson above." />
      ) : (
        <Table
          caption="Lessons"
          columns={[
            { key: "lesson", label: "Lesson" },
            { key: "duration", label: "Minutes" },
            { key: "done", label: "Completed" },
            { key: "actions", label: "Actions" },
          ]}
          rows={lessons.map((l) => ({
            lesson: <span className="row-title">{l.title}</span>,
            duration: String(l.durationMinutes),
            done: l.completed ? "Yes" : "—",
            actions: l.completed ? null : (
              <Button size="small" onClick={() => void completeLesson(l.id)}>Mark complete</Button>
            ),
          }))}
        />
      )}
      <h3>Students ({enrolments.length})</h3>
      {enrolments.length === 0 ? (
        <p className="row-sub">No enrolments yet.</p>
      ) : (
        <Table
          caption="Enrolments"
          columns={[
            { key: "student", label: "Student" },
            { key: "status", label: "Status" },
            { key: "progress", label: "Progress" },
            { key: "enrolled", label: "Enrolled" },
          ]}
          rows={enrolments.map((e) => ({
            student: (
              <span>
                <span className="row-title">{e.displayName || e.login}</span>
                <span className="row-sub">@{e.login}</span>
              </span>
            ),
            status: <Badge tone={e.status === "completed" ? "teal" : "amber"}>{e.status}</Badge>,
            progress: `${e.lessonsCompleted}/${e.lessonsTotal} lessons`,
            enrolled: formatDateTime(e.enrolledAt, timezone),
          }))}
        />
      )}
      <h3>Question banks ({banks.length})</h3>
      <div className="toolbar">
        <Input label="Bank title" value={bankTitle} onChange={(e) => setBankTitle(e.target.value)} placeholder="e.g. Week 1" />
        <Button icon="plus" disabled={bankTitle.trim() === ""} onClick={() => void addBank()}>
          Add bank
        </Button>
      </div>
      {banks.map((b) => (
        <p key={b.id}>
          <Button size="small" onClick={() => setSelectedBank(selectedBank?.id === b.id ? null : b)}>
            {b.title} ({b.questionCount} questions)
          </Button>
        </p>
      ))}
      {selectedBank !== null ? (
        <BankDetail bank={selectedBank} onError={onError} />
      ) : null}
      <h3>Exams ({exams.length})</h3>
      <div className="toolbar">
        <Input label="Exam title" value={examTitle} onChange={(e) => setExamTitle(e.target.value)} placeholder="e.g. Week 1 Quiz" />
        <Select
          label="Question bank"
          value={examBankId}
          onChange={(e) => setExamBankId(e.target.value)}
        >
          <option value="">Choose a bank…</option>
          {banks.map((b) => (
            <option key={b.id} value={String(b.id)}>{b.title}</option>
          ))}
        </Select>
        <Button icon="plus" disabled={examTitle.trim() === "" || examBankId === ""} onClick={() => void addExam()}>
          Add exam
        </Button>
      </div>
      {exams.length === 0 ? (
        <p className="row-sub">No exams yet. Exams default to 30 minutes, 1 attempt, 50% pass mark — edit them from the API or a later pass.</p>
      ) : (
        <Table
          caption="Exams"
          columns={[
            { key: "exam", label: "Exam" },
            { key: "duration", label: "Minutes" },
            { key: "attempts", label: "Attempts" },
            { key: "pass", label: "Pass mark" },
            { key: "status", label: "Status" },
            { key: "actions", label: "Actions" },
          ]}
          rows={exams.map((x) => ({
            exam: <span className="row-title">{x.title}</span>,
            duration: String(x.durationMinutes),
            attempts: x.maxAttempts === 0 ? "Unlimited" : String(x.maxAttempts),
            pass: `${x.passMarkPercent}%`,
            status: <Badge tone={x.status === "published" ? "teal" : "amber"}>{x.status}</Badge>,
            actions: (
              <Button size="small" onClick={() => void publishExam(x)}>
                {x.status === "published" ? "Unpublish" : "Publish"}
              </Button>
            ),
          }))}
        />
      )}
    </Card>
  );
}

function BankDetail({ bank, onError }: { bank: SchoolQuestionBank; onError: (msg: string) => void }): React.ReactElement {
  const [questions, setQuestions] = useState<SchoolQuestion[]>([]);
  const [qtype, setQtype] = useState("mcq");
  const [text, setText] = useState("");
  const [options, setOptions] = useState("");
  const [answer, setAnswer] = useState("");
  const [marks, setMarks] = useState("1");

  const load = useCallback(async () => {
    try {
      const res = await api.get<{ items: SchoolQuestion[] }>(`/api/school/banks/${bank.id}/questions`);
      setQuestions(res.items);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load questions.");
    }
  }, [bank.id, onError]);

  useEffect(() => {
    void load();
  }, [load]);

  const add = async (): Promise<void> => {
    if (text.trim() === "") return;
    try {
      const opts = options.split(",").map((o) => o.trim()).filter((o) => o !== "");
      let parsedAnswer: unknown = null;
      if (qtype === "mcq") parsedAnswer = Number(answer);
      else if (qtype === "true_false") parsedAnswer = answer.toLowerCase() === "true";
      else if (qtype === "multi_select")
        parsedAnswer = answer.split(",").map((a) => Number(a.trim())).filter((n) => !Number.isNaN(n));
      await api.post(`/api/school/banks/${bank.id}/questions`, {
        type: qtype,
        questionText: text,
        options: opts,
        answer: parsedAnswer,
        modelAnswer: qtype === "short_answer" ? answer : "",
        marks: Number(marks) || 1,
      });
      setText("");
      setOptions("");
      setAnswer("");
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to add question.");
    }
  };

  return (
    <Card title={`Questions — ${bank.title}`} sub={`${questions.length} questions`}>
      <div className="toolbar">
        <Select
          label="Type"
          value={qtype}
          onChange={(e) => setQtype(e.target.value)}
        >
          <option value="mcq">Multiple choice</option>
          <option value="true_false">True / False</option>
          <option value="multi_select">Multi-select</option>
          <option value="short_answer">Short answer</option>
        </Select>
        <Input label="Question" value={text} onChange={(e) => setText(e.target.value)} placeholder="Question text…" />
        <Input
          label="Options (comma separated)"
          value={options}
          onChange={(e) => setOptions(e.target.value)}
          placeholder="e.g. 3, 4, 5"
        />
        <Input
          label={qtype === "mcq" ? "Correct option index (0-based)" : qtype === "true_false" ? "Answer (true/false)" : qtype === "multi_select" ? "Correct indices (comma separated)" : "Model answer (for the grader)"}
          value={answer}
          onChange={(e) => setAnswer(e.target.value)}
          placeholder={qtype === "short_answer" ? "e.g. Abuja" : "e.g. 1"}
        />
        <Input label="Marks" value={marks} onChange={(e) => setMarks(e.target.value)} placeholder="1" />
        <Button icon="plus" disabled={text.trim() === ""} onClick={() => void add()}>
          Add question
        </Button>
      </div>
      {questions.length === 0 ? (
        <EmptyState icon="book" title="No questions yet" description="Add the first question above. Short answers are graded by a teacher, never automatically." />
      ) : (
        <Table
          caption="Questions"
          columns={[
            { key: "q", label: "Question" },
            { key: "type", label: "Type" },
            { key: "marks", label: "Marks" },
            { key: "topic", label: "Topic" },
          ]}
          rows={questions.map((q) => ({
            q: <span className="row-title">{q.questionText}</span>,
            type: q.type.replace("_", " "),
            marks: String(q.marks),
            topic: q.topic || "—",
          }))}
        />
      )}
    </Card>
  );
}

function MyExamsTab({ onError }: { onError: (msg: string) => void }): React.ReactElement {
  const [courses, setCourses] = useState<SchoolCourseList | null>(null);
  const [selectedCourse, setSelectedCourse] = useState<SchoolCourse | null>(null);
  const [exams, setExams] = useState<SchoolExam[]>([]);
  const [attempt, setAttempt] = useState<SchoolAttempt | null>(null);
  const [answers, setAnswers] = useState<Record<number, unknown>>({});
  const [result, setResult] = useState<SchoolResult | null>(null);
  const [nowMs, setNowMs] = useState(Date.now());

  const loadCourses = useCallback(async () => {
    try {
      setCourses(await api.get<SchoolCourseList>("/api/school/courses?page=1&per_page=20"));
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load courses.");
    }
  }, [onError]);

  useEffect(() => {
    void loadCourses();
  }, [loadCourses]);

  useEffect(() => {
    const t = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);

  const openCourse = async (c: SchoolCourse): Promise<void> => {
    setSelectedCourse(c);
    setAttempt(null);
    setResult(null);
    try {
      const res = await api.get<{ items: SchoolExam[] }>(`/api/school/courses/${c.id}/exams`);
      setExams(res.items);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load exams.");
    }
  };

  const start = async (exam: SchoolExam): Promise<void> => {
    try {
      const a = await api.post<SchoolAttempt>(`/api/school/exams/${exam.id}/start`, {});
      setAttempt(a);
      setResult(null);
      const saved: Record<number, unknown> = {};
      for (const ans of a.answers) {
        if (ans.answer !== null) saved[ans.questionId] = ans.answer;
      }
      setAnswers(saved);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to start exam.");
    }
  };

  const saveAnswer = async (questionId: number, value: unknown): Promise<void> => {
    if (attempt === null) return;
    setAnswers((prev) => ({ ...prev, [questionId]: value }));
    try {
      const a = await api.post<SchoolAttempt>(`/api/school/attempts/${attempt.id}/answers`, {
        questionId,
        answer: value,
      });
      setAttempt(a);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to save answer.");
    }
  };

  const submit = async (): Promise<void> => {
    if (attempt === null) return;
    try {
      const a = await api.post<SchoolAttempt>(`/api/school/attempts/${attempt.id}/submit`, {});
      setAttempt(a);
      try {
        const r = await api.get<SchoolResult>(`/api/school/attempts/${attempt.id}/result`);
        setResult(r);
      } catch {
        setResult(null);
      }
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to submit exam.");
    }
  };

  const remainingMs = attempt !== null ? Math.max(0, attempt.deadlineMs - nowMs) : 0;
  const remainingMin = Math.floor(remainingMs / 60000);
  const remainingSec = Math.floor((remainingMs % 60000) / 1000);

  return (
    <div>
      <Card title="My exams" sub="Enrol in a course, then sit its exams. The timer is enforced by the server — the clock here is only a display.">
        {courses === null ? (
          <Skeleton height={120} label="Loading courses" />
        ) : courses.items.length === 0 ? (
          <EmptyState icon="book" title="No courses yet" description="Enrol in a published course from the Courses tab first." />
        ) : (
          <div className="toolbar">
            {courses.items.map((c) => (
              <Button key={c.id} size="small" onClick={() => void openCourse(c)}>
                {c.title}
              </Button>
            ))}
          </div>
        )}
        {selectedCourse !== null ? (
          <div>
            <h3>{selectedCourse.title} — exams</h3>
            {exams.length === 0 ? (
              <p className="row-sub">No published exams for this course yet.</p>
            ) : (
              <Table
                caption="Exams"
                columns={[
                  { key: "exam", label: "Exam" },
                  { key: "duration", label: "Minutes" },
                  { key: "attempts", label: "Attempts used" },
                  { key: "pass", label: "Pass mark" },
                  { key: "actions", label: "Actions" },
                ]}
                rows={exams.map((x) => ({
                  exam: <span className="row-title">{x.title}</span>,
                  duration: String(x.durationMinutes),
                  attempts: `${x.attemptsUsed}${x.maxAttempts === 0 ? "" : ` / ${x.maxAttempts}`}`,
                  pass: `${x.passMarkPercent}%`,
                  actions: (
                    <Button size="small" onClick={() => void start(x)}>
                      Start exam
                    </Button>
                  ),
                }))}
              />
            )}
          </div>
        ) : null}
      </Card>
      {attempt !== null ? (
        <Card
          title={`Exam attempt — ${attempt.status}`}
          sub={`Time left: ${remainingMin}:${String(remainingSec).padStart(2, "0")} (server deadline) · Score so far: ${attempt.score}/${attempt.maxScore}`}
        >
          {attempt.status === "in_progress" ? (
            <div>
              {attempt.questions.map((q) => (
                <div key={q.questionId} style={{ marginBottom: 16 }}>
                  <p>
                    <strong>{q.questionText}</strong> <span className="row-sub">({q.marks} mark(s))</span>
                  </p>
                  {q.type === "short_answer" ? (
                    <div className="field">
                      <label htmlFor={`answer-${q.questionId}`}>Your answer</label>
                      <textarea
                        id={`answer-${q.questionId}`}
                        value={String(answers[q.questionId] ?? "")}
                        onChange={(e) => void saveAnswer(q.questionId, e.target.value)}
                        rows={3}
                        placeholder="Type your answer…"
                      />
                    </div>
                  ) : q.type === "true_false" ? (
                    <Select
                      label="Answer"
                      value={answers[q.questionId] === true ? "true" : answers[q.questionId] === false ? "false" : ""}
                      onChange={(e) => void saveAnswer(q.questionId, e.target.value === "true")}
                    >
                      <option value="">Choose…</option>
                      <option value="true">True</option>
                      <option value="false">False</option>
                    </Select>
                  ) : q.type === "multi_select" ? (
                    <div>
                      {q.options.map((opt, i) => {
                        const current = Array.isArray(answers[q.questionId])
                          ? (answers[q.questionId] as number[])
                          : [];
                        const checked = current.includes(i);
                        return (
                          <label key={i} style={{ display: "block" }}>
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={() => {
                                const next = checked
                                  ? current.filter((x) => x !== i)
                                  : [...current, i];
                                void saveAnswer(q.questionId, next);
                              }}
                            />{" "}
                            {opt}
                          </label>
                        );
                      })}
                    </div>
                  ) : (
                    <Select
                      label="Answer"
                      value={answers[q.questionId] === undefined ? "" : String(answers[q.questionId] as number)}
                      onChange={(e) => void saveAnswer(q.questionId, Number(e.target.value))}
                    >
                      <option value="">Choose…</option>
                      {q.options.map((opt, i) => (
                        <option key={i} value={String(i)}>{opt}</option>
                      ))}
                    </Select>
                  )}
                </div>
              ))}
              <Button onClick={() => void submit()}>Submit exam</Button>
            </div>
          ) : (
            <div>
              <p>
                Attempt {attempt.status}. Score: {attempt.score}/{attempt.maxScore}.
              </p>
              {attempt.answers.map((a) => {
                const q = attempt.questions.find((x) => x.questionId === a.questionId);
                return (
                  <p key={a.questionId}>
                    <strong>{q?.questionText ?? `Question ${a.questionId}`}</strong>
                    <br />
                    Your answer: {typeof a.answer === "string" ? a.answer : JSON.stringify(a.answer)}
                    <br />
                    {a.isCorrect === null
                      ? "Awaiting grading."
                      : a.isCorrect
                        ? `Correct — ${a.marksAwarded ?? 0} mark(s).`
                        : `Not correct — ${a.marksAwarded ?? 0} mark(s).`}
                    {a.graderFeedback ? <span> Feedback: {a.graderFeedback}</span> : null}
                  </p>
                );
              })}
              {result !== null ? (
                <p>
                  Result: {result.percent}% — {result.passed ? "Passed." : "Not passed."}
                  {result.certificateCode !== null ? (
                    <span>
                      {" "}
                      Certificate:{" "}
                      <a href={`/school/certificates/${result.certificateCode}`} target="_blank" rel="noreferrer">
                        {result.certificateCode}
                      </a>
                    </span>
                  ) : null}
                </p>
              ) : attempt.status === "awaiting_grading" ? (
                <p className="row-sub">Short answers are being graded by a teacher. Check back soon.</p>
              ) : null}
            </div>
          )}
        </Card>
      ) : null}
    </div>
  );
}

function GradingTab({ timezone, onError }: { timezone: string; onError: (msg: string) => void }): React.ReactElement {
  const [items, setItems] = useState<SchoolGradingItem[]>([]);
  const [marks, setMarks] = useState<Record<number, string>>({});
  const [feedback, setFeedback] = useState<Record<number, string>>({});

  const load = useCallback(async () => {
    try {
      const res = await api.get<{ items: SchoolGradingItem[] }>("/api/school/grading-queue");
      setItems(res.items);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load grading queue.");
    }
  }, [onError]);

  useEffect(() => {
    void load();
  }, [load]);

  const grade = async (item: SchoolGradingItem): Promise<void> => {
    const awarded = Number(marks[item.answerId] ?? "");
    if (Number.isNaN(awarded)) return;
    try {
      await api.post(`/api/school/answers/${item.answerId}/grade`, {
        marksAwarded: awarded,
        feedback: feedback[item.answerId] ?? "",
      });
      await load();
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to grade answer.");
    }
  };

  return (
    <Card title="Grading queue" sub={`${items.length} short answers awaiting grading`}>
      {items.length === 0 ? (
        <EmptyState icon="check" title="Nothing to grade" description="Short-answer questions appear here once a student submits them." />
      ) : (
        <Table
          caption="Grading queue"
          columns={[
            { key: "q", label: "Question" },
            { key: "student", label: "Student" },
            { key: "answer", label: "Answer" },
            { key: "model", label: "Model answer" },
            { key: "grade", label: "Grade" },
          ]}
          rows={items.map((it) => (
            {
              q: (
                <span>
                  <span className="row-title">{it.questionText}</span>
                  <span className="row-sub">
                    {it.examTitle} · {it.marks} mark(s) · submitted {formatDateTime(new Date(it.submittedAtMs).toISOString(), timezone)}
                  </span>
                </span>
              ),
              student: `${it.studentName || it.studentLogin}`,
              answer: typeof it.answer === "string" ? it.answer : JSON.stringify(it.answer),
              model: it.modelAnswer || "—",
              grade: (
                <span>
                  <Input
                    label={`Marks (0–${it.marks})`}
                    value={marks[it.answerId] ?? ""}
                    onChange={(e) => setMarks((prev) => ({ ...prev, [it.answerId]: e.target.value }))}
                    placeholder="0"
                  />
                  <Input
                    label="Feedback"
                    value={feedback[it.answerId] ?? ""}
                    onChange={(e) => setFeedback((prev) => ({ ...prev, [it.answerId]: e.target.value }))}
                    placeholder="Optional feedback"
                  />
                  <Button size="small" onClick={() => void grade(it)}>
                    Grade
                  </Button>
                </span>
              ),
            }
          ))}
        />
      )}
    </Card>
  );
}

function CertificatesTab({ timezone, onError }: { timezone: string; onError: (msg: string) => void }): React.ReactElement {
  const [items, setItems] = useState<SchoolCertificate[]>([]);

  const load = useCallback(async () => {
    try {
      const res = await api.get<{ items: SchoolCertificate[] }>("/api/school/certificates");
      setItems(res.items);
    } catch (err) {
      onError(err instanceof Error ? err.message : "Failed to load certificates.");
    }
  }, [onError]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <Card title="Certificates" sub={`${items.length} certificates`}>
      {items.length === 0 ? (
        <EmptyState icon="award" title="No certificates yet" description="Certificates issue automatically when a student passes an exam at or above the pass mark." />
      ) : (
        <Table
          caption="Certificates"
          columns={[
            { key: "code", label: "Code" },
            { key: "student", label: "Student" },
            { key: "course", label: "Course" },
            { key: "score", label: "Score" },
            { key: "issued", label: "Issued" },
          ]}
          rows={items.map((c) => ({
            code: (
              <a href={`/school/certificates/${c.code}`} target="_blank" rel="noreferrer">
                {c.code}
              </a>
            ),
            student: c.studentName || c.studentLogin,
            course: c.courseTitle,
            score: `${c.score}/${c.maxScore} (${c.percent}%)`,
            issued: formatDateTime(c.issuedAt, timezone),
          }))}
        />
      )}
    </Card>
  );
}
