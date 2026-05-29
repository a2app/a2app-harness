export interface StandardApp {
  description: string;
  splashBody: string;
}

export const STANDARD_APPS: Record<string, StandardApp> = {
  todo: {
    description:
      "Well-written task list app with add, toggle, delete, and clear-completed flows.",
    splashBody: `let todos = [
    {text: "Buy groceries" tag: "errands" done: false}
    {text: "Write unit tests" tag: "dev" done: false}
]
let max_todos = 5

fn remaining_count(){
    let count = 0
    for todo in todos {
        if !todo.done count += 1
    }
    count
}

fn sync_status(){
    ui.todo_status.set_text(remaining_count() + " remaining / " + todos.len() + " total (5 slots)")
}

fn sync_row_0(){
    if 0 < todos.len() {
        let todo = todos[0]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_0.marker.set_text(marker)
        ui.todo_row_0.label.set_text(todo.text)
        ui.todo_row_0.tag.set_text(todo.tag)
    } else {
        ui.todo_row_0.marker.set_text(".")
        ui.todo_row_0.label.set_text("Empty slot")
        ui.todo_row_0.tag.set_text("")
    }
}

fn sync_row_1(){
    if 1 < todos.len() {
        let todo = todos[1]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_1.marker.set_text(marker)
        ui.todo_row_1.label.set_text(todo.text)
        ui.todo_row_1.tag.set_text(todo.tag)
    } else {
        ui.todo_row_1.marker.set_text(".")
        ui.todo_row_1.label.set_text("Empty slot")
        ui.todo_row_1.tag.set_text("")
    }
}

fn sync_row_2(){
    if 2 < todos.len() {
        let todo = todos[2]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_2.marker.set_text(marker)
        ui.todo_row_2.label.set_text(todo.text)
        ui.todo_row_2.tag.set_text(todo.tag)
    } else {
        ui.todo_row_2.marker.set_text(".")
        ui.todo_row_2.label.set_text("Empty slot")
        ui.todo_row_2.tag.set_text("")
    }
}

fn sync_row_3(){
    if 3 < todos.len() {
        let todo = todos[3]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_3.marker.set_text(marker)
        ui.todo_row_3.label.set_text(todo.text)
        ui.todo_row_3.tag.set_text(todo.tag)
    } else {
        ui.todo_row_3.marker.set_text(".")
        ui.todo_row_3.label.set_text("Empty slot")
        ui.todo_row_3.tag.set_text("")
    }
}

fn sync_row_4(){
    if 4 < todos.len() {
        let todo = todos[4]
        let marker = "[ ]"
        if todo.done { marker = "[x]" }
        ui.todo_row_4.marker.set_text(marker)
        ui.todo_row_4.label.set_text(todo.text)
        ui.todo_row_4.tag.set_text(todo.tag)
    } else {
        ui.todo_row_4.marker.set_text(".")
        ui.todo_row_4.label.set_text("Empty slot")
        ui.todo_row_4.tag.set_text("")
    }
}

fn sync_rows(){
    sync_row_0()
    sync_row_1()
    sync_row_2()
    sync_row_3()
    sync_row_4()
    sync_status()
}

fn add_todo(text){
    let clean = ("" + text).trim()
    if clean == "" { return }
    if todos.len() >= max_todos {
        ui.todo_status.set_text("List is full (5 slots max)")
        return
    }
    todos.push({text: clean tag: "" done: false})
    ui.todo_input.set_text("")
    sync_rows()
}

fn toggle_todo(index){
    if index >= todos.len() { return }
    let next_done = !todos[index].done
    todos[index] += {done: next_done}
    sync_rows()
}

fn delete_todo(index){
    if index >= todos.len() { return }
    todos.remove(index)
    sync_rows()
}

fn clear_done(){
    todos.retain(|todo| !todo.done)
    sync_rows()
}

let TodoRow = RoundedView{
    width: Fill height: Fit
    padding: Inset{top: 8 bottom: 8 left: 12 right: 12}
    flow: Right spacing: 10
    align: Align{y: 0.5}
    new_batch: true
    draw_bg.color: #x2a2a3a
    draw_bg.border_radius: 8.0
    marker := Label{text: "[ ]" width: 24 draw_text.color: #x8fb7ff draw_text.text_style.font_size: 11}
    label := Label{text: "task" width: Fill draw_text.color: #ddd draw_text.text_style.font_size: 11}
    tag := Label{text: "" draw_text.color: #888 draw_text.text_style.font_size: 9}
    toggle := ButtonFlatter{text: "Toggle" width: 56 height: 28 draw_text.color: #9fb1d8}
    delete := ButtonFlatter{text: "Delete" width: 56 height: 28 draw_text.color: #888}
}

RoundedView{
    width: Fill height: Fit
    flow: Down spacing: 10
    padding: 16
    new_batch: true
    draw_bg.color: #x1e1e2e
    draw_bg.border_radius: 10.0
    Label{text: "My Tasks" draw_text.color: #fff draw_text.text_style.font_size: 14}
    View{
        width: Fill height: Fit
        flow: Right spacing: 8
        align: Align{y: 0.5}
        todo_input := TextInput{
            width: Fill height: 34
            empty_text: "Add a new task"
            on_return: |text| add_todo(text)
        }
        Button{text: "Add" width: 64 height: 34 on_click: || add_todo(ui.todo_input.text())}
    }
    View{
        width: Fill height: Fit
        flow: Down spacing: 4
        todo_row_0 := TodoRow{
            label.text: "Buy groceries"
            tag.text: "errands"
            toggle.on_click: || toggle_todo(0)
            delete.on_click: || delete_todo(0)
        }
        todo_row_1 := TodoRow{
            label.text: "Write unit tests"
            tag.text: "dev"
            toggle.on_click: || toggle_todo(1)
            delete.on_click: || delete_todo(1)
        }
        todo_row_2 := TodoRow{
            marker.text: "."
            label.text: "Empty slot"
            tag.text: ""
            toggle.on_click: || toggle_todo(2)
            delete.on_click: || delete_todo(2)
        }
        todo_row_3 := TodoRow{
            marker.text: "."
            label.text: "Empty slot"
            tag.text: ""
            toggle.on_click: || toggle_todo(3)
            delete.on_click: || delete_todo(3)
        }
        todo_row_4 := TodoRow{
            marker.text: "."
            label.text: "Empty slot"
            tag.text: ""
            toggle.on_click: || toggle_todo(4)
            delete.on_click: || delete_todo(4)
        }
    }
    View{
        width: Fill height: Fit
        flow: Right
        align: Align{y: 0.5}
        todo_status := Label{text: "2 remaining / 2 total (5 slots)" width: Fill draw_text.color: #aaa}
        ButtonFlatter{text: "Clear completed" on_click: || clear_done()}
    }
}`,
  },
  notes: {
    description:
      "Well-written quick notes app with add, delete, and clear-all flows.",
    splashBody: `let notes = [
    {text: "Pick up dry cleaning"}
    {text: "Outline release checklist"}
]
let max_notes = 5

fn sync_status(){
    ui.note_status.set_text(notes.len() + " notes (5 slots)")
}

fn sync_row_0(){
    if 0 < notes.len() {
        ui.note_row_0.label.set_text(notes[0].text)
    } else {
        ui.note_row_0.label.set_text("Empty slot")
    }
}

fn sync_row_1(){
    if 1 < notes.len() {
        ui.note_row_1.label.set_text(notes[1].text)
    } else {
        ui.note_row_1.label.set_text("Empty slot")
    }
}

fn sync_row_2(){
    if 2 < notes.len() {
        ui.note_row_2.label.set_text(notes[2].text)
    } else {
        ui.note_row_2.label.set_text("Empty slot")
    }
}

fn sync_row_3(){
    if 3 < notes.len() {
        ui.note_row_3.label.set_text(notes[3].text)
    } else {
        ui.note_row_3.label.set_text("Empty slot")
    }
}

fn sync_row_4(){
    if 4 < notes.len() {
        ui.note_row_4.label.set_text(notes[4].text)
    } else {
        ui.note_row_4.label.set_text("Empty slot")
    }
}

fn sync_rows(){
    sync_row_0()
    sync_row_1()
    sync_row_2()
    sync_row_3()
    sync_row_4()
    sync_status()
}

fn add_note(text){
    let clean = ("" + text).trim()
    if clean == "" { return }
    if notes.len() >= max_notes {
        ui.note_status.set_text("List is full (5 slots max)")
        return
    }
    notes.push({text: clean})
    ui.note_input.set_text("")
    sync_rows()
}

fn delete_note(index){
    if index >= notes.len() { return }
    notes.remove(index)
    sync_rows()
}

fn clear_all(){
    notes.retain(|note| false)
    sync_rows()
}

let NoteRow = RoundedView{
    width: Fill height: Fit
    padding: Inset{top: 8 bottom: 8 left: 12 right: 12}
    flow: Right spacing: 10
    align: Align{y: 0.5}
    new_batch: true
    draw_bg.color: #x2a2a3a
    draw_bg.border_radius: 8.0
    label := Label{text: "note" width: Fill draw_text.color: #ddd draw_text.text_style.font_size: 11}
    delete := ButtonFlatter{text: "Delete" width: 56 height: 28 draw_text.color: #888}
}

RoundedView{
    width: Fill height: Fit
    flow: Down spacing: 10
    padding: 16
    new_batch: true
    draw_bg.color: #x1e1e2e
    draw_bg.border_radius: 10.0
    Label{text: "Quick Notes" draw_text.color: #fff draw_text.text_style.font_size: 14}
    View{
        width: Fill height: Fit
        flow: Right spacing: 8
        align: Align{y: 0.5}
        note_input := TextInput{
            width: Fill height: 34
            empty_text: "Write something down"
            on_return: |text| add_note(text)
        }
        Button{text: "Add" width: 64 height: 34 on_click: || add_note(ui.note_input.text())}
    }
    View{
        width: Fill height: Fit
        flow: Down spacing: 4
        note_row_0 := NoteRow{
            label.text: "Pick up dry cleaning"
            delete.on_click: || delete_note(0)
        }
        note_row_1 := NoteRow{
            label.text: "Outline release checklist"
            delete.on_click: || delete_note(1)
        }
        note_row_2 := NoteRow{
            label.text: "Empty slot"
            delete.on_click: || delete_note(2)
        }
        note_row_3 := NoteRow{
            label.text: "Empty slot"
            delete.on_click: || delete_note(3)
        }
        note_row_4 := NoteRow{
            label.text: "Empty slot"
            delete.on_click: || delete_note(4)
        }
    }
    View{
        width: Fill height: Fit
        flow: Right
        align: Align{y: 0.5}
        note_status := Label{text: "2 notes (5 slots)" width: Fill draw_text.color: #aaa}
        ButtonFlatter{text: "Clear all" on_click: || clear_all()}
    }
}`,
  },
};
