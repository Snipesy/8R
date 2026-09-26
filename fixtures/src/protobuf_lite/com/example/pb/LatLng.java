package com.example.pb;

import com.google.protobuf.GeneratedMessageLite;

/** The shape protoc's lite codegen gives message LatLng. */
public final class LatLng extends GeneratedMessageLite {
    public static final int LATITUDE_FIELD_NUMBER = 1;
    public static final int LONGITUDE_FIELD_NUMBER = 2;
    private double latitude_;
    private double longitude_;
    public LatLng(double latitude, double longitude) {
        this.latitude_ = latitude;
        this.longitude_ = longitude;
    }
    @Override protected String describe() { return "L:" + latitude_ + "," + longitude_; }
}
