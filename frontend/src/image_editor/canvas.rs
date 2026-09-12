use leptos::{ev, html, leptos_dom::helpers::window_event_listener, prelude::*};
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, PointerEvent, WheelEvent};

use super::gestures::{GestureAction, Gestures};
use crate::image_editor::{DrawTool, DrawingObject, EditorDraft, Point, RenderedCanvas, Viewport};

/// 编辑窗口使用的画笔画布；每次完整笔画只提交一次历史操作。
#[component]
pub fn ImageEditorCanvas(
    #[prop(optional)] base: Option<RwSignal<Option<std::rc::Rc<super::EditorBase>>, LocalStorage>>,
    #[prop(optional)] imported_mask: Option<
        RwSignal<Option<std::rc::Rc<super::EditorMask>>, LocalStorage>,
    >,
    draft: RwSignal<EditorDraft>,
    erase: RwSignal<bool>,
    color: RwSignal<String>,
    brush_width: RwSignal<f64>,
    on_object: Callback<DrawingObject>,
    on_error: Callback<String>,
    #[prop(optional)] on_drawing: Option<Callback<bool>>,
    #[prop(optional)] reset_view: Option<RwSignal<u64>>,
    tool: RwSignal<DrawTool>,
    selected: RwSignal<Option<String>>,
    on_move: Callback<(String, Point)>,
    on_text: Callback<Point>,
) -> impl IntoView {
    let canvas = NodeRef::<html::Canvas>::new();
    let stage = NodeRef::<html::Div>::new();
    let points = RwSignal::new(Vec::<Point>::new());
    let drawing = RwSignal::new(false);
    let sample_limit_reported = RwSignal::new(false);
    let space_down = RwSignal::new(false);
    let gestures = StoredValue::new(Gestures::default());
    let viewport = RwSignal::new(None::<Viewport>);
    let active_tool = RwSignal::new(DrawTool::Pen);
    let fit = move || {
        let Some(element) = stage.get_untracked() else {
            return;
        };
        let bounds = element.get_bounding_client_rect();
        let draft = draft.read_untracked();
        if let Ok(value) = Viewport::fit(draft.width, draft.height, bounds.width(), bounds.height())
        {
            viewport.set(Some(value));
        }
    };
    Effect::new(move |_| {
        stage.track();
        if let Some(reset) = reset_view {
            reset.track();
        }
        fit();
    });
    let relative_point = move |client_x: i32, client_y: i32| {
        stage.get_untracked().map(|element| {
            let bounds = element.get_bounding_client_rect();
            Point {
                x: f64::from(client_x) - bounds.left(),
                y: f64::from(client_y) - bounds.top(),
            }
        })
    };
    Effect::new(move |_| {
        let Some(canvas) = canvas.get() else { return };
        let preview = draft.read();
        let current_points = points.get();
        let live_object = active_tool
            .get()
            .geometry(current_points, erase.get())
            .map(|geometry| DrawingObject {
                id: "live-stroke-preview".into(),
                geometry,
                color: color.get(),
                width: brush_width.get(),
            });
        let result = (|| -> Result<(), String> {
            let base = base.and_then(|base| base.get());
            let imported_mask = imported_mask
                .and_then(|mask| mask.get())
                .filter(|mask| preview.imported_mask_asset_id.as_deref() == Some(mask.id()));
            let rendered = RenderedCanvas::render_editor_preview(
                &preview,
                base.as_ref().map(|base| base.image()),
                imported_mask.as_ref().map(|mask| mask.image()),
                1024,
                live_object.as_ref(),
            )?;
            canvas.set_width(rendered.canvas().width());
            canvas.set_height(rendered.canvas().height());
            let context = canvas
                .get_context("2d")
                .map_err(|error| format!("{error:?}"))?
                .ok_or("无法创建画布上下文。")?
                .dyn_into::<CanvasRenderingContext2d>()
                .map_err(|_| "画布上下文类型错误。")?;
            context
                .draw_image_with_html_canvas_element(rendered.canvas(), 0.0, 0.0)
                .map_err(|error| format!("画布预览失败：{error:?}"))?;
            Ok(())
        })();
        if let Err(error) = result {
            on_error.run(error);
        }
    });
    on_cleanup(move || {
        if let Some(canvas) = canvas.get_untracked() {
            canvas.set_width(0);
            canvas.set_height(0);
        }
    });
    let finish = move || {
        if !drawing.get_untracked() {
            return;
        }
        drawing.set(false);
        if let Some(callback) = on_drawing {
            callback.run(false);
        }
        let completed = points.get_untracked();
        points.set(Vec::new());
        if completed.is_empty() {
            return;
        }
        if active_tool.get_untracked() == DrawTool::Text {
            on_text.run(completed[0]);
            return;
        }
        if active_tool.get_untracked() == DrawTool::Select {
            if let Some(id) = selected.get_untracked() {
                let end = completed[completed.len() - 1];
                on_move.run((
                    id,
                    Point {
                        x: end.x - completed[0].x,
                        y: end.y - completed[0].y,
                    },
                ));
            }
            return;
        }
        let Some(geometry) = active_tool
            .get_untracked()
            .geometry(completed, erase.get_untracked())
        else {
            return;
        };
        on_object.run(DrawingObject {
            id: uuid::Uuid::new_v4().to_string(),
            geometry,
            color: color.get_untracked(),
            width: brush_width.get_untracked(),
        });
    };
    let handle_action = move |action: GestureAction| match action {
        GestureAction::None => (),
        GestureAction::Commit => finish(),
        GestureAction::Cancel => {
            points.set(Vec::new());
            drawing.set(false);
            if let Some(callback) = on_drawing {
                callback.run(false);
            }
        }
        GestureAction::Begin(screen) | GestureAction::Append(screen) => {
            let Some(view) = viewport.get_untracked() else {
                return;
            };
            let point = view.to_image(screen);
            let draft = draft.read_untracked();
            let point = Point {
                x: point.x.clamp(0.0, f64::from(draft.width)),
                y: point.y.clamp(0.0, f64::from(draft.height)),
            };
            if matches!(action, GestureAction::Begin(_)) {
                active_tool.set(tool.get_untracked());
                if tool.get_untracked() == DrawTool::Select {
                    let context = canvas
                        .get_untracked()
                        .and_then(|canvas| canvas.get_context("2d").ok().flatten())
                        .and_then(|context| context.dyn_into::<CanvasRenderingContext2d>().ok());
                    let found = draft
                        .object_at(&view, screen, 6.0, |text, size| {
                            context
                                .as_ref()
                                .and_then(|context| {
                                    context.set_font(&format!("{size}px sans-serif"));
                                    context
                                        .measure_text(text)
                                        .ok()
                                        .map(|metrics| metrics.width())
                                })
                                .unwrap_or(0.0)
                        })
                        .map(|object| object.id.clone());
                    selected.set(found);
                }
                sample_limit_reported.set(false);
                points.set(vec![point]);
                drawing.set(true);
                if let Some(callback) = on_drawing {
                    callback.run(true);
                }
            } else {
                if points.with_untracked(Vec::len) >= 16_384
                    && !sample_limit_reported.get_untracked()
                {
                    sample_limit_reported.set(true);
                    on_error.run("当前笔画已达到采样上限，请抬起后继续绘制下一笔。".into());
                }
                points.update(|points| {
                    if active_tool.get_untracked() != DrawTool::Pen {
                        points.truncate(1);
                        points.push(point);
                        return;
                    }
                    if points.len() < 16_384
                        && points
                            .last()
                            .is_none_or(|last| (last.x - point.x).hypot(last.y - point.y) >= 0.5)
                    {
                        points.push(point);
                    }
                });
            }
        }
    };
    let update_pointer = move |event: &PointerEvent| {
        let Some(point) = relative_point(event.client_x(), event.client_y()) else {
            return;
        };
        let mut action = GestureAction::None;
        viewport.update(|view| {
            if let Some(view) = view {
                gestures.update_value(|gestures| {
                    action = gestures.update(event.pointer_id(), point, view)
                });
            }
        });
        handle_action(action);
    };
    let resize_listener = window_event_listener(ev::resize, move |_| {
        // 视口变化时取消尚未完成的笔画，防止把两套坐标拼成一笔。
        handle_action(GestureAction::Cancel);
        gestures.set_value(Gestures::default());
        fit();
    });
    on_cleanup(move || resize_listener.remove());
    view! {
        <div node_ref=stage tabindex="0" aria-label="编辑画布；滚轮缩放，空格加拖拽平移，双指缩放"
            style="position:relative;width:100%;height:100%;min-height:120px;overflow:hidden;touch-action:none"
            on:keydown=move |event: web_sys::KeyboardEvent| {
                if event.code() == "Space" { event.prevent_default(); space_down.set(true); }
            }
            on:keyup=move |event: web_sys::KeyboardEvent| {
                if event.code() == "Space" { event.prevent_default(); space_down.set(false); }
            }
            on:blur=move |_| space_down.set(false)
            on:wheel=move |event: WheelEvent| {
                event.prevent_default();
                if gestures.with_value(Gestures::active) { return; }
                if let Some(point) = relative_point(event.client_x(), event.client_y()) {
                    let delta = event.delta_y() * match event.delta_mode() { 1 => 16.0, 2 => 400.0, _ => 1.0 };
                    viewport.update(|view| { if let Some(view) = view { view.zoom_at(point, (-delta * 0.002).clamp(-2.0, 2.0).exp()); } });
                }
            }
            on:pointerdown=move |event: PointerEvent| {
                if event.button() != 0 && event.button() != 1 { return; }
                if let Some(point) = relative_point(event.client_x(), event.client_y()) {
                    event.prevent_default();
                    if let Some(element) = stage.get_untracked() {
                        let _ = element.focus();
                        let _ = element.set_pointer_capture(event.pointer_id());
                    }
                    let mut action = GestureAction::None;
                    let outside = viewport.get_untracked().is_some_and(|view| {
                        let image = view.to_image(point);
                        draft.with_untracked(|draft| image.x < 0.0 || image.y < 0.0 || image.x > f64::from(draft.width) || image.y > f64::from(draft.height))
                    });
                    gestures.update_value(|gestures| action = gestures.begin(event.pointer_id(), point, outside || space_down.get_untracked() || event.button() == 1));
                    handle_action(action);
                }
            }
            on:pointermove=move |event: PointerEvent| update_pointer(&event)
            on:pointerup=move |event: PointerEvent| {
                update_pointer(&event);
                let mut action = GestureAction::None;
                gestures.update_value(|gestures| action = gestures.end(event.pointer_id(), false));
                handle_action(action);
                if let Some(element) = stage.get_untracked() { let _ = element.release_pointer_capture(event.pointer_id()); }
            }
            on:pointercancel=move |event: PointerEvent| {
                let mut action = GestureAction::None;
                gestures.update_value(|gestures| action = gestures.end(event.pointer_id(), true));
                handle_action(action);
            }
            on:lostpointercapture=move |event: PointerEvent| {
                let mut action = GestureAction::None;
                gestures.update_value(|gestures| action = gestures.end(event.pointer_id(), true));
                handle_action(action);
            }
        >
            <canvas node_ref=canvas class="image-editor-canvas" aria-label="图像编辑画布"
                style=move || {
                    let draft = draft.read();
                    let (offset, scale) = viewport.get().map(|view| (view.offset(), view.scale()))
                        .unwrap_or((Point { x: 0.0, y: 0.0 }, 1.0));
                    format!("position:absolute;left:0;top:0;max-width:none;max-height:none;pointer-events:none;transform-origin:0 0;width:{}px;height:{}px;transform:translate({}px,{}px) scale({});", draft.width, draft.height, offset.x, offset.y, scale)
                }
            />
        </div>
    }
}
