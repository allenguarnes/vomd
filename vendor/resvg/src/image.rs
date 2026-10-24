// Copyright 2018 the Resvg Authors
// SPDX-License-Identifier: Apache-2.0 OR MIT

pub fn render(
    image: &usvg::Image,
    ctx: &crate::render::Context,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    if !image.is_visible() {
        return;
    }

    render_inner(
        image.kind(),
        image.size(),
        ctx,
        transform,
        image.rendering_mode(),
        pixmap,
    );
}

pub fn render_inner(
    image_kind: &usvg::ImageKind,
    size: tiny_skia::Size,
    ctx: &crate::render::Context,
    transform: tiny_skia::Transform,
    #[allow(unused_variables)] rendering_mode: usvg::ImageRendering,
    pixmap: &mut tiny_skia::PixmapMut,
) {
    match image_kind {
        usvg::ImageKind::SVG(tree) => {
            render_vector(tree, ctx, transform, pixmap);
        }
        // VOMD: the hook's pixels, drawn as `raster-images` draws its own.
        _ => {
            if let Some(raster) = (ctx.hooks.raster)(image_kind, transform, rendering_mode) {
                raster_images::draw(size, raster, transform, rendering_mode, pixmap);
            }
        }
    }
}

fn render_vector(
    tree: &usvg::Tree,
    ctx: &crate::render::Context,
    transform: tiny_skia::Transform,
    pixmap: &mut tiny_skia::PixmapMut,
) -> Option<()> {
    let mut sub_pixmap = tiny_skia::Pixmap::new(pixmap.width(), pixmap.height()).unwrap();
    crate::render_with(tree, transform, &mut sub_pixmap.as_mut(), ctx.hooks);
    pixmap.draw_pixmap(
        0,
        0,
        sub_pixmap.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        tiny_skia::Transform::default(),
        None,
    );

    Some(())
}

mod raster_images {
    use usvg::ImageRendering;

    // VOMD: `raster` is the hook's, of any size, scaled to the image's `size`.
    pub(crate) fn draw(
        size: tiny_skia::Size,
        raster: tiny_skia::Pixmap,
        transform: tiny_skia::Transform,
        rendering_mode: usvg::ImageRendering,
        pixmap: &mut tiny_skia::PixmapMut,
    ) -> Option<()> {
        let rect = size.to_rect(0.0, 0.0)?;
        let scale = tiny_skia::Transform::from_scale(
            size.width() / raster.width() as f32,
            size.height() / raster.height() as f32,
        );

        let quality = match rendering_mode {
            ImageRendering::OptimizeQuality => tiny_skia::FilterQuality::Bicubic,
            ImageRendering::OptimizeSpeed => tiny_skia::FilterQuality::Nearest,
            ImageRendering::Smooth => tiny_skia::FilterQuality::Bilinear,
            ImageRendering::HighQuality => tiny_skia::FilterQuality::Bicubic,
            ImageRendering::CrispEdges => tiny_skia::FilterQuality::Nearest,
            ImageRendering::Pixelated => tiny_skia::FilterQuality::Nearest,
        };

        let pattern = tiny_skia::Pattern::new(
            raster.as_ref(),
            tiny_skia::SpreadMode::Pad,
            quality,
            1.0,
            scale,
        );
        let mut paint = tiny_skia::Paint::default();
        paint.shader = pattern;

        // VOMD: a rectangle too far for tiny-skia's scan is clipped to the pixmap (see
        // `geom::fill`); `fill_rect` with a transform fills the rectangle's path.
        let path = tiny_skia::PathBuilder::from_rect(rect);
        match crate::geom::fill(&path, transform, pixmap.width(), pixmap.height()) {
            crate::geom::Drawing::Whole => {
                pixmap.fill_rect(rect, &paint, transform, None);
            }
            crate::geom::Drawing::Device(path) => {
                paint.shader.transform(transform);
                pixmap.fill_path(
                    &path,
                    &paint,
                    tiny_skia::FillRule::Winding,
                    tiny_skia::Transform::identity(),
                    None,
                );
            }
            crate::geom::Drawing::Nothing => {}
        }

        Some(())
    }
}
